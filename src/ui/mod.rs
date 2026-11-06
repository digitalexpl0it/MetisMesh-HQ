mod cli_page;
mod emoji_catalog;
mod help_page;
mod icons;
mod infra_page;
mod map_host;
mod map_page;
mod messages_rail;
mod overview_repeaters;
mod tracker_page;
mod weather_page;
mod welcome;
mod widgets;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{Local, TimeZone, Timelike};

use crate::timefmt::{self, DateFormatPref, TimeFormatPref};
use dioxus::prelude::*;

use crate::contact_qr::{
    contact_add_uri, enqueue_contact_qr, qr_data_url_from_file, qr_public_url,
};
use crate::db::{unix_now, ChannelRow, Db, DmThreadRow, SystemChannel, TelemetryPoint};
use crate::radio::{
    channel_sender_and_body, prefix_channel_sender, BleRadio, RadioHandle, RadioStatus,
};
use crate::radio_presets::{
    community_presets, loc_policy_label, preset_label_for, queue_path_hash_mode,
    refresh_presets_cache_if_needed, RadioPreset, BW_OPTIONS_KHZ, CR_OPTIONS, SF_OPTIONS,
};
use crate::secrets::Secrets;
use crate::signup::Enrollment;

use cli_page::CliPage;
use help_page::HelpPage;
use icons::{Icon, IconName};
use infra_page::InfraPage;
use map_page::MapPage;
use tracker_page::TrackerPage;
use weather_page::WeatherPage;
use welcome::{reopen_welcome, welcome_is_done, WelcomeFinishAction, WelcomeWizard};
use widgets::{
    append_emoji, clamp_mesh_text, conversation_avatar_tone, format_channel_share,
    format_position_share, heard_icon_class, heard_within_week, insert_mesh_snippet, Badge, BarMix,
    BarMixRow, BatteryChart, BatteryPill, BatterySample, Button, ButtonKind, Card, ChartSeries,
    ComposePlusAction, ComposePlusButton, ConversationAvatar, Dialog, EmojiPickerButton, LineChart,
    MeshTextCount, PageHeading, PinSlideDown, PopoverDismissLayer, SelectInput, SlideDownPanel,
    Sparkline, TextInput, Throbber, ToastHost, ToggleSwitch, MESH_TEXT_MAX_BYTES,
};

/// Show a transient toast (and log it). Prefer this over inline amber labels.
fn status_note(message: impl Into<String>) {
    let message = message.into();
    let kind = crate::toast::infer_kind(&message);
    crate::toast::push(kind, message);
}

pub struct AppState {
    pub db: Arc<Db>,
    pub secrets: Arc<Secrets>,
    pub radio: RadioHandle,
}

static STATE: OnceLock<AppState> = OnceLock::new();

pub fn install(state: AppState) {
    let _ = STATE.set(state);
}

pub(crate) fn state() -> &'static AppState {
    STATE.get().expect("application state")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Dashboard,
    Map,
    Weather,
    Tracker,
    Infrastructure,
    Roster,
    Inbox,
    Messages,
    Cli,
    Help,
    Settings,
}

/// Sidebar jump index: page title + keywords (no live roster/message bodies).
struct NavSearchEntry {
    page: Page,
    title: &'static str,
    icon: IconName,
    /// Topic labels shown as “in Page · topic” when they match.
    topics: &'static [&'static str],
}

const NAV_SEARCH_INDEX: &[NavSearchEntry] = &[
    NavSearchEntry {
        page: Page::Dashboard,
        title: "Overview",
        icon: IconName::Dashboard,
        topics: &[
            "dashboard",
            "status",
            "companion",
            "radio pool",
            "alerts",
            "backup",
            "firmware",
            "enrolled",
            "heard",
            "traffic",
            "battery",
            "signup queue",
            "repeaters",
        ],
    },
    NavSearchEntry {
        page: Page::Messages,
        title: "Messages",
        icon: IconName::Messages,
        topics: &[
            "Channels",
            "Direct",
            "direct messages",
            "DM",
            "METIS-INFO",
            "METIS-NET",
            "chat",
            "unread",
            "channel secret",
        ],
    },
    NavSearchEntry {
        page: Page::Inbox,
        title: "Inbox",
        icon: IconName::Inbox,
        topics: &[
            "Waiting",
            "signup",
            "pending signup",
            "approve",
            "deny",
            "Pending alerts",
            "alerts",
            "weather alert",
            "quake",
            "amber",
            "NWS",
        ],
    },
    NavSearchEntry {
        page: Page::Roster,
        title: "Roster",
        icon: IconName::Users,
        topics: &[
            "nodes",
            "contacts",
            "claim",
            "enroll",
            "revoke",
            "invite channel",
            "heard",
            "enrolled",
            "denied",
            "GPS",
            "public key",
            "advert",
        ],
    },
    NavSearchEntry {
        page: Page::Map,
        title: "Map",
        icon: IconName::Map,
        topics: &[
            "Tracked",
            "Heard",
            "pins",
            "trails",
            "site pin",
            "sonar",
            "location",
            "MapLibre",
            "offline map",
            "PMTiles",
            "basemap",
        ],
    },
    NavSearchEntry {
        page: Page::Weather,
        title: "Weather",
        icon: IconName::Weather,
        topics: &[
            "station",
            "Ecowitt",
            "GW1200",
            "WittBoy",
            "MQTT",
            "temperature",
            "humidity",
            "yard",
            "local weather",
        ],
    },
    NavSearchEntry {
        page: Page::Infrastructure,
        title: "Infrastructure",
        icon: IconName::Signal,
        topics: &[
            "repeater",
            "room",
            "sensor",
            "claim",
            "Claim from roster",
            "verify",
            "Verify admin",
            "admin password",
            "Status probe",
            "Discover repeaters",
            "remote CLI",
            "ACL",
            "neighbors",
            "Verified",
            "Unverified",
            "Unclaim",
        ],
    },
    NavSearchEntry {
        page: Page::Tracker,
        title: "Tracker",
        icon: IconName::Track,
        topics: &[
            "track",
            "whitelist",
            "vehicle",
            "hike",
            "GPS share",
            "breadcrumb",
            "trail",
            "Tracked",
        ],
    },
    NavSearchEntry {
        page: Page::Cli,
        title: "CLI",
        icon: IconName::Terminal,
        topics: &[
            "device CLI",
            "companion CLI",
            "remote CLI",
            "reboot",
            "get radio",
            "firmware commands",
            "terminal",
        ],
    },
    NavSearchEntry {
        page: Page::Help,
        title: "Help",
        icon: IconName::Help,
        topics: &[
            "help",
            "how to",
            "signup",
            "enroll",
            "enrollment",
            "approve",
            "deny",
            "revoke",
            "channel",
            "secret",
            "METIS-INFO",
            "METIS-NET",
            "weather",
            "flasher",
            "firmware",
            "backup",
            "WAL",
            "offline",
            "runbook",
            "radio users",
        ],
    },
    NavSearchEntry {
        page: Page::Settings,
        title: "Settings",
        icon: IconName::Settings,
        topics: &[
            // Top-level settings tabs
            "HQ",
            "Device",
            "Flasher",
            "Diagnostics",
            "About",
            "version",
            "license",
            "GitHub",
            "setup wizard",
            "welcome",
            // HQ subtabs
            "Connect",
            "Preferences",
            "Services",
            "Data",
            // Connect link tabs
            "Bluetooth",
            "USB",
            "Network",
            "TCP",
            "Wi-Fi",
            "Ethernet",
            "companion PIN",
            "primary radio",
            "pair",
            "reconnect",
            // Device subtabs
            "Identity",
            "Radio",
            "Advanced",
            "clock",
            "advert",
            "name",
            // Preferences / secrets / history
            "secrets",
            "channel secret",
            "purge",
            "history",
            "timezone",
            "date",
            "time",
            "brand",
            "logo",
            // Services
            "weather",
            "MQTT",
            "Mosquitto",
            "Ecowitt",
            "GW1200",
            "WittBoy",
            "custom server",
            "Local API",
            "bearer",
            "Home Assistant",
            "NWS",
            "quake",
            "amber",
            "ZIP",
            "site pin",
            // Data
            "backup",
            "restore",
            "metisbak",
            "contacts export",
            "offline map",
            // Flasher / diagnostics
            "firmware",
            "flash",
            "ESP32",
            "serial",
            "logs",
            "diagnostics log",
        ],
    },
];

fn nav_search_hits(query: &str) -> Vec<(i32, Page, IconName, String, String)> {
    let needle = query.trim();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for entry in NAV_SEARCH_INDEX {
        let title_score = fuzzy_score(needle, &[entry.title]);
        let mut best_topic: Option<(&'static str, i32)> = None;
        for topic in entry.topics {
            let Some(score) = fuzzy_score(needle, &[topic]) else {
                continue;
            };
            if best_topic.is_none_or(|(_, best)| score > best) {
                best_topic = Some((topic, score));
            }
        }
        let score = match (title_score, best_topic.map(|(_, s)| s)) {
            (Some(a), Some(b)) => a.max(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => continue,
        };
        let (label, subtitle) = match best_topic {
            Some((topic, topic_score))
                if title_score.is_none_or(|ts| topic_score >= ts)
                    && !entry.title.eq_ignore_ascii_case(topic) =>
            {
                (
                    format!("{} · {}", entry.title, topic),
                    format!("Open {topic} in {}", entry.title),
                )
            }
            _ => (
                entry.title.to_string(),
                format!("Go to {}", entry.title),
            ),
        };
        hits.push((score, entry.page, entry.icon, label, subtitle));
    }
    hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.3.cmp(&b.3)));
    hits.dedup_by(|a, b| a.1 == b.1);
    hits.truncate(12);
    hits
}

fn reset_main_scroll() {
    // `.hq-main` is shared across pages — leaving Help (etc.) scrolled down
    // would paint the next page below the viewport until the operator scrolls.
    let _ = document::eval(
        r#"const main = document.querySelector('.hq-main');
           if (main) main.scrollTop = 0;"#,
    );
}

#[allow(non_snake_case)]
pub fn App() -> Element {
    let mut page = use_signal(|| Page::Dashboard);
    let mut nav_open = use_signal(|| false);
    let mut nav_query = use_signal(String::new);
    let mut pin_open = use_signal(|| false);
    let mut pin_target = use_signal(String::new);
    let mut pin_error = use_signal(String::new);
    let mut pin_form_key = use_signal(|| 0u64);
    let mut tick = use_signal(|| 0u64);
    let mut toast_tick = use_signal(|| 0u32);
    let mut show_welcome = use_signal(|| !welcome_is_done());
    let mut open_about_after_welcome = use_signal(|| false);
    // Bumped when the operator picks Messages in the nav so an open thread
    // returns to the Channels / Direct root instead of staying drilled in.
    let mut messages_root_epoch = use_signal(|| 0u64);
    let mut started = use_signal(|| false);
    // Reset shared main-pane scroll whenever the active page changes.
    use_effect(move || {
        let _ = page();
        reset_main_scroll();
        spawn(async move {
            // Second pass after the new page paints (tall Help → short Settings).
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            reset_main_scroll();
        });
    });
    if !started() {
        started.set(true);
        // Undo sticky WebView page-zoom from trackpad pinch (map gestures).
        #[cfg(feature = "desktop")]
        {
            dioxus::desktop::window().set_zoom_level(1.0);
        }
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                tick.set(tick() + 1);
            }
        });
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                crate::toast::prune_expired();
                toast_tick.with_mut(|n| *n = n.wrapping_add(1));
            }
        });
    }
    let _ = tick();
    let toast_frame = toast_tick();
    let radio = state().radio.snapshot();
    let pending_signups = state().db.pending_nodes().unwrap_or_default().len();
    let pending_alerts = state()
        .db
        .list_pending_service_events()
        .unwrap_or_default()
        .len();
    let inbox_badge = (pending_signups + pending_alerts) as u32;
    let roster_badge = state()
        .db
        .count_unnoticed_nodes()
        .unwrap_or(0)
        .max(0) as u32;
    let channel_unread = state()
        .db
        .total_channel_unread()
        .unwrap_or(0)
        .max(0) as u32;
    let dm_unread = state()
        .db
        .total_dm_unread()
        .unwrap_or(0)
        .max(0) as u32;
    let channel_mention_nav = state()
        .db
        .list_channels()
        .unwrap_or_default()
        .iter()
        .any(|c| c.unread_mentions > 0);
    let dm_mention_nav = state()
        .db
        .list_dm_threads()
        .unwrap_or_default()
        .iter()
        .any(|t| t.unread_mentions > 0);
    let expanded = nav_open();
    let nav_width = if expanded { "w-56" } else { "w-[4.5rem]" };
    let brand_logo = crate::brand::custom_logo_url(&state().db);
    // Settings → "Run setup again" clears welcome_done; reopen the overlay.
    if !welcome_is_done() && !show_welcome() {
        show_welcome.set(true);
    }

    rsx! {
        document::Title { "Metis Mesh HQ" }
        // Desktop inlines these in with_custom_head so the first paint is styled.
        // Web still needs <link> injection here.
        if !cfg!(feature = "desktop") {
            document::Stylesheet { href: asset!("/assets/tailwind.css") }
            document::Stylesheet { href: asset!("/assets/form.css") }
        }
        div { class: "hq-shell bg-background text-foreground",
            PinSlideDown {
                open: pin_open(),
                target: pin_target(),
                error: pin_error(),
                form_key: pin_form_key(),
                on_close: move |_| {
                    pin_open.set(false);
                    pin_error.set(String::new());
                },
                on_submit: move |raw: String| {
                    match crate::ble_pair::parse_meshcore_pin(&raw) {
                        Ok(pin) => {
                            let target = pin_target();
                            pin_open.set(false);
                            pin_error.set(String::new());
                            state().radio.pair(target, pin);
                        }
                        Err(err) => pin_error.set(err),
                    }
                },
            }
            div { class: "hq-body",
                nav { class: "flex {nav_width} shrink-0 flex-col border-r border-border bg-card transition-[width] duration-200",
                    div { class: "flex items-center gap-3 border-b border-border px-3 py-3",
                        button {
                            class: "inline-flex h-11 w-11 shrink-0 items-center justify-center rounded-md text-signal hover:bg-accent cursor-pointer",
                            title: if expanded { "Collapse navigation" } else { "Expand navigation" },
                            onclick: move |_| {
                                let next = !nav_open();
                                nav_open.set(next);
                                if !next {
                                    nav_query.set(String::new());
                                }
                            },
                            Icon {
                                name: if expanded { IconName::PanelClose } else { IconName::Menu },
                                size: 22,
                            }
                        }
                        if expanded {
                            div { class: "min-w-0",
                                p { class: "text-[0.65rem] uppercase tracking-[0.2em] text-muted-foreground", "Metis" }
                                h1 { class: "truncate text-base font-semibold", "Mesh HQ" }
                            }
                        }
                    }
                    div { class: "flex min-h-0 flex-1 flex-col gap-1 p-2",
                        if expanded {
                            div { class: "mb-1",
                                TextInput {
                                    value: nav_query(),
                                    placeholder: "Find a page…".to_string(),
                                    kind: "text".to_string(),
                                    oninput: move |event: FormEvent| nav_query.set(event.value()),
                                }
                            }
                        }
                        {
                            let searching = expanded && !nav_query().trim().is_empty();
                            let hits = if searching {
                                nav_search_hits(&nav_query())
                            } else {
                                Vec::new()
                            };
                            if searching {
                                rsx! {
                                    div { class: "flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto",
                                        if hits.is_empty() {
                                            p { class: "px-2 py-3 text-xs text-muted-foreground",
                                                "No pages match."
                                            }
                                        } else {
                                            for (_score, hit_page, icon, title, subtitle) in hits {
                                                {
                                                    let go = hit_page;
                                                    rsx! {
                                                        button {
                                                            class: if page() == go {
                                                                "flex w-full cursor-pointer items-start gap-3 rounded-md bg-accent px-3 py-2 text-left text-signal"
                                                            } else {
                                                                "flex w-full cursor-pointer items-start gap-3 rounded-md px-3 py-2 text-left text-muted-foreground hover:bg-muted hover:text-foreground"
                                                            },
                                                            onclick: move |_| {
                                                                if go == Page::Messages {
                                                                    messages_root_epoch
                                                                        .set(messages_root_epoch() + 1);
                                                                }
                                                                page.set(go);
                                                                nav_query.set(String::new());
                                                            },
                                                            span { class: "mt-0.5 shrink-0",
                                                                Icon { name: icon, size: 20 }
                                                            }
                                                            span { class: "min-w-0",
                                                                span { class: "block truncate text-sm font-medium", "{title}" }
                                                                span { class: "block truncate text-[0.7rem] text-muted-foreground", "{subtitle}" }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                rsx! {
                                    NavButton {
                                        icon: IconName::Dashboard,
                                        label: "Overview".to_string(),
                                        expanded,
                                        active: page() == Page::Dashboard,
                                        onclick: move |_| page.set(Page::Dashboard),
                                    }
                                    NavButton {
                                        icon: IconName::Messages,
                                        label: "Messages".to_string(),
                                        expanded,
                                        active: page() == Page::Messages,
                                        badge: channel_unread,
                                        badge_alt: dm_unread,
                                        badge_mention: channel_mention_nav,
                                        badge_alt_mention: dm_mention_nav,
                                        onclick: move |_| {
                                            messages_root_epoch.set(messages_root_epoch() + 1);
                                            page.set(Page::Messages);
                                        },
                                    }
                                    NavButton {
                                        icon: IconName::Inbox,
                                        label: "Inbox".to_string(),
                                        expanded,
                                        active: page() == Page::Inbox,
                                        badge: inbox_badge,
                                        onclick: move |_| page.set(Page::Inbox),
                                    }
                                    NavButton {
                                        icon: IconName::Users,
                                        label: "Roster".to_string(),
                                        expanded,
                                        active: page() == Page::Roster,
                                        badge: roster_badge,
                                        onclick: move |_| page.set(Page::Roster),
                                    }
                                    NavButton {
                                        icon: IconName::Map,
                                        label: "Map".to_string(),
                                        expanded,
                                        active: page() == Page::Map,
                                        onclick: move |_| page.set(Page::Map),
                                    }
                                    NavButton {
                                        icon: IconName::Weather,
                                        label: "Weather".to_string(),
                                        expanded,
                                        active: page() == Page::Weather,
                                        onclick: move |_| page.set(Page::Weather),
                                    }
                                    NavButton {
                                        icon: IconName::Signal,
                                        label: "Infrastructure".to_string(),
                                        expanded,
                                        active: page() == Page::Infrastructure,
                                        onclick: move |_| page.set(Page::Infrastructure),
                                    }
                                    NavButton {
                                        icon: IconName::Track,
                                        label: "Tracker".to_string(),
                                        expanded,
                                        active: page() == Page::Tracker,
                                        onclick: move |_| page.set(Page::Tracker),
                                    }
                                    NavButton {
                                        icon: IconName::Terminal,
                                        label: "CLI".to_string(),
                                        expanded,
                                        active: page() == Page::Cli,
                                        onclick: move |_| page.set(Page::Cli),
                                    }
                                    div { class: "mt-auto" }
                                    NavButton {
                                        icon: IconName::Help,
                                        label: "Help".to_string(),
                                        expanded,
                                        active: page() == Page::Help,
                                        onclick: move |_| page.set(Page::Help),
                                    }
                                    NavButton {
                                        icon: IconName::Settings,
                                        label: "Settings".to_string(),
                                        expanded,
                                        active: page() == Page::Settings,
                                        onclick: move |_| page.set(Page::Settings),
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "hq-right",
                    // Not role=banner: some WebKit/Wry embeds treat that landmark as a
                    // fixed overlay so Messages/Weather paint under the top bar.
                    div {
                        class: "hq-topbar",
                        aria_label: "Headquarters status",
                        div { class: "flex min-w-0 items-center",
                            if let Some(src) = brand_logo.as_ref() {
                                img {
                                    class: "h-9 max-w-[14rem] object-contain object-left",
                                    src: "{src}",
                                    alt: "HQ brand",
                                }
                            } else {
                                img {
                                    class: "h-9 max-w-[14rem] object-contain object-left",
                                    src: "{crate::brand::default_logo_url()}",
                                    alt: "EmergHQ",
                                }
                            }
                        }
                        HeaderRadioControls { radio: radio.clone() }
                    }
                    div {
                        class: if page() == Page::Map {
                            "hq-main hq-main-map"
                        } else if page() == Page::Messages {
                            "hq-main hq-main-fill"
                        } else {
                            "hq-main"
                        },
                        role: "main",
                        match page() {
                            Page::Dashboard => rsx! { DashboardPage { radio: radio.clone(), on_navigate: move |next| page.set(next) } },
                            Page::Map => rsx! { MapPage {} },
                            Page::Weather => rsx! { WeatherPage {} },
                            Page::Tracker => rsx! { TrackerPage {} },
                            Page::Infrastructure => rsx! { InfraPage { radio: radio.clone() } },
                            Page::Roster => rsx! { RosterPage {} },
                            Page::Inbox => rsx! { InboxPage {} },
                            // key remounts Messages on each nav click → root list, no mid-render signal writes.
                            Page::Messages => rsx! { MessagesPage { key: "{messages_root_epoch()}" } },
                            Page::Cli => rsx! { CliPage { radio: radio.clone() } },
                            Page::Help => rsx! { HelpPage {} },
                            Page::Settings => rsx! {
                                SettingsPage {
                                    radio: radio.clone(),
                                    prefer_about: open_about_after_welcome(),
                                    on_about_consumed: move |_| open_about_after_welcome.set(false),
                                    on_open_help: move |_| page.set(Page::Help),
                                    on_request_pin: move |target: String| {
                                        let target = target.trim().to_string();
                                        if target.is_empty() {
                                            pin_error.set("Enter the Bluetooth name or address first.".into());
                                            pin_target.set(String::new());
                                        } else {
                                            pin_error.set(String::new());
                                            pin_target.set(target);
                                        }
                                        pin_form_key.set(pin_form_key() + 1);
                                        pin_open.set(true);
                                    },
                                }
                            },
                        }
                    }
                }
            }
            if show_welcome() {
                WelcomeWizard {
                    radio: radio.clone(),
                    on_finished: move |action| {
                        show_welcome.set(false);
                        if action == WelcomeFinishAction::OpenAbout {
                            open_about_after_welcome.set(true);
                            page.set(Page::Settings);
                        }
                    },
                    on_request_pin: move |target: String| {
                        let target = target.trim().to_string();
                        if target.is_empty() {
                            pin_error.set("Enter the Bluetooth name or address first.".into());
                            pin_target.set(String::new());
                        } else {
                            pin_error.set(String::new());
                            pin_target.set(target);
                        }
                        pin_form_key.set(pin_form_key() + 1);
                        pin_open.set(true);
                    },
                }
            }
            ToastHost { tick: toast_frame }
            footer { class: "flex items-center gap-4 border-t border-border px-4 py-2 text-xs text-muted-foreground",
                span { class: "inline-flex shrink-0 items-center gap-2",
                    Icon { name: status_icon(&radio.state), size: 14 }
                    "{status_label(&radio.state)}"
                }
                div { class: "flex min-w-0 flex-1 flex-col gap-1",
                    if radio.sync_phase.is_active() {
                        {
                            let pct = radio
                                .sync_progress
                                .map(|(done, total)| {
                                    if total == 0 {
                                        0
                                    } else {
                                        ((done as f64 / total as f64) * 100.0).round() as i64
                                    }
                                });
                            rsx! {
                                div { class: "h-1.5 w-full overflow-hidden rounded-full bg-muted",
                                    if let Some(pct) = pct {
                                        div {
                                            class: "h-full rounded-full bg-signal transition-[width] duration-200",
                                            style: "width: {pct}%",
                                        }
                                    } else {
                                        div {
                                            class: "h-full w-2/5 animate-pulse rounded-full bg-signal",
                                        }
                                    }
                                }
                            }
                        }
                    }
                    span { class: "flex min-w-0 items-center gap-2 truncate",
                        if radio.sync_phase.is_active() && !radio.sync_label.is_empty() {
                            span { class: "truncate", "{radio.sync_label}" }
                        } else {
                            span { class: "truncate", "{radio.detail}" }
                        }
                        if radio.sync_phase.is_active() {
                            if let Some((done, total)) = radio.sync_progress {
                                span {
                                    class: "shrink-0 tabular-nums text-foreground",
                                    "{done}/{total}"
                                }
                            }
                        }
                    }
                }
                if radio.sync_phase.is_discover() {
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| state().radio.cancel_mesh_scan(),
                        "Stop"
                    }
                }
                if let Some(pct) = radio.battery_pct {
                    span { class: "shrink-0 text-foreground", "{pct}%" }
                }
            }
        }
    }
}

fn status_icon(state: &str) -> IconName {
    match state {
        "connected" => IconName::Radio,
        "connecting" | "reconnecting" | "scanning" | "scanning_usb" | "pairing" | "disconnecting" => {
            IconName::Scan
        }
        "error" => IconName::Close,
        _ => IconName::Radio,
    }
}

fn transport_icon(transport: &str) -> IconName {
    match transport {
        "usb" => IconName::Usb,
        "ble" => IconName::Bluetooth,
        "tcp" => IconName::Signal,
        _ => IconName::Radio,
    }
}

fn transport_label(transport: &str) -> &'static str {
    match transport {
        "usb" => "USB",
        "ble" => "Bluetooth",
        "tcp" => "Network",
        _ => "Companion",
    }
}

fn transport_badge(transport: &str) -> &'static str {
    match transport {
        "usb" => "USB",
        "ble" => "BLE",
        "tcp" => "TCP",
        _ => "Radio",
    }
}

/// Short age for overview chips (`12m ago`, `3h ago`, `2d ago`).
fn format_age_secs(at: i64) -> String {
    let age = (unix_now() - at).max(0);
    if age < 60 {
        "just now".into()
    } else if age < 3600 {
        format!("{}m ago", age / 60)
    } else if age < 86_400 {
        format!("{}h ago", age / 3600)
    } else {
        format!("{}d ago", age / 86_400)
    }
}

fn status_label(state: &str) -> &'static str {
    match state {
        "connected" => "Connected",
        "connecting" => "Connecting",
        "reconnecting" => "Reconnecting",
        "scanning" => "Scanning",
        "scanning_usb" => "Refreshing USB",
        "pairing" => "Pairing",
        "disconnecting" => "Disconnecting",
        "error" => "Radio error",
        _ => "Disconnected",
    }
}

fn format_nav_badge(count: u32) -> String {
    if count > 99 {
        "99+".into()
    } else {
        count.to_string()
    }
}

#[component]
fn NavButton(
    icon: IconName,
    label: String,
    expanded: bool,
    active: bool,
    #[props(default)] badge: u32,
    /// Second badge (e.g. DM unread beside channel unread on Messages).
    #[props(default)] badge_alt: u32,
    /// When true, the primary badge uses the mention tone (HQ @mentioned).
    #[props(default)] badge_mention: bool,
    #[props(default)] badge_alt_mention: bool,
    onclick: EventHandler<MouseEvent>,
) -> Element {
    let class = if active {
        "relative flex items-center gap-3 rounded-md bg-accent text-signal"
    } else {
        "relative flex items-center gap-3 rounded-md text-muted-foreground hover:bg-muted hover:text-foreground"
    };
    let layout = if expanded {
        "h-11 justify-start px-3"
    } else {
        "h-12 w-full justify-center px-0"
    };
    let show_badges = badge > 0 || badge_alt > 0;
    let badge_label = format_nav_badge(badge);
    let badge_alt_label = format_nav_badge(badge_alt);
    let badge_class = if badge_mention {
        "nav-count-badge nav-count-badge-mention"
    } else {
        "nav-count-badge"
    };
    let badge_alt_class = if badge_alt_mention {
        "nav-count-badge nav-count-badge-alt nav-count-badge-mention"
    } else {
        "nav-count-badge nav-count-badge-alt"
    };
    rsx! {
        button {
            class: "{class} {layout} cursor-pointer",
            title: "{label}",
            onclick: move |event| onclick.call(event),
            if active {
                span { class: "absolute left-0 top-2 bottom-2 w-0.5 rounded-full bg-signal" }
            }
            span { class: "relative inline-flex shrink-0",
                Icon { name: icon, size: 24 }
                if !expanded && show_badges {
                    span { class: "absolute -right-2 -top-1.5 flex items-center gap-0.5",
                        if badge > 0 {
                            span { class: "{badge_class}", "{badge_label}" }
                        }
                        if badge_alt > 0 {
                            span { class: "{badge_alt_class}", "{badge_alt_label}" }
                        }
                    }
                }
            }
            if expanded {
                span { class: "min-w-0 flex-1 truncate text-sm font-medium", "{label}" }
                if show_badges {
                    span { class: "ml-auto flex shrink-0 items-center gap-1",
                        if badge > 0 {
                            span { class: "{badge_class}", "{badge_label}" }
                        }
                        if badge_alt > 0 {
                            span { class: "{badge_alt_class}", "{badge_alt_label}" }
                        }
                    }
                }
            }
        }
    }
}

fn format_time(seconds: Option<i64>) -> String {
    let Some(seconds) = seconds else {
        return "—".into();
    };
    timefmt::format_stamp(&state().db, seconds)
}

fn format_clock(seconds: i64) -> String {
    timefmt::format_clock(&state().db, seconds)
}

#[component]
fn DetailField(label: String, children: Element) -> Element {
    rsx! {
        div { class: "min-w-0",
            p { class: "mb-0.5 text-[0.65rem] uppercase tracking-[0.14em] text-muted-foreground", "{label}" }
            div { class: "text-sm text-foreground", {children} }
        }
    }
}

fn node_title(name: &str, requested: &Option<String>) -> String {
    if name.trim().is_empty() {
        requested.clone().unwrap_or_else(|| "Unnamed".into())
    } else if let Some(requested) = requested {
        if requested != name {
            format!("{name} ({requested})")
        } else {
            name.to_string()
        }
    } else {
        name.to_string()
    }
}

fn looks_like_bt_address(value: &str) -> bool {
    let trimmed = value.trim();
    let parts: Vec<_> = trimmed.split([':', '-']).collect();
    parts.len() == 6
        && parts.iter().all(|part| {
            part.len() == 2 && part.chars().all(|ch| ch.is_ascii_hexdigit())
        })
}

fn companion_display_name(row: &crate::db::RadioRow, devices: &[BleRadio]) -> String {
    let label = row.label.trim();
    if !label.is_empty() && label != "Companion" && !looks_like_bt_address(label) {
        return label.to_string();
    }
    if let Some(device) = devices
        .iter()
        .find(|device| device.address.eq_ignore_ascii_case(&row.address))
    {
        let name = device.name.trim();
        if !name.is_empty() && name != "Unnamed device" && !looks_like_bt_address(name) {
            return name.to_string();
        }
    }
    if !label.is_empty() && label != "Companion" {
        return label.to_string();
    }
    row.address.clone()
}

fn companion_option_label(row: &crate::db::RadioRow, devices: &[BleRadio]) -> String {
    let name = companion_display_name(row, devices);
    match row.transport.as_str() {
        "usb" => format!("{name} · USB"),
        "ble" => format!("{name} · BLE"),
        "tcp" => format!("{name} · TCP"),
        _ => name,
    }
}

/// Always-visible companion control in the page header.
/// Disconnected: optional saved-radio picker + Connect.
/// Connected: status pill + Disconnect.
#[component]
fn HeaderRadioControls(radio: RadioStatus) -> Element {
    let saved_radios = state().db.list_saved_radios().unwrap_or_default();
    let preferred = state()
        .db
        .setting("radio_name")
        .ok()
        .flatten()
        .unwrap_or_default();
    let default_target = if saved_radios.iter().any(|row| row.address == preferred) {
        preferred.clone()
    } else {
        saved_radios
            .first()
            .map(|row| row.address.clone())
            .unwrap_or_default()
    };
    let mut selected = use_signal(|| default_target.clone());
    // Keep the picker on the preferred radio when settings change.
    if !default_target.is_empty()
        && selected().is_empty()
        && saved_radios.iter().any(|row| row.address == default_target)
    {
        selected.set(default_target.clone());
    }
    let linked = radio.state == "connected";
    let busy = matches!(
        radio.state.as_str(),
        "connecting" | "reconnecting" | "pairing" | "scanning" | "scanning_usb" | "disconnecting"
    );
    let show_live = linked || busy || !radio.connected_name.is_empty();
    let selected_transport = saved_radios
        .iter()
        .find(|row| row.address == selected())
        .map(|row| row.transport.as_str())
        .unwrap_or("");
    let connect_icon = if !radio.transport.is_empty() {
        transport_icon(&radio.transport)
    } else {
        transport_icon(selected_transport)
    };

    if show_live {
        let label = if !radio.connected_name.is_empty() {
            radio.connected_name.clone()
        } else {
            selected()
        };
        let live_icon = if linked && !radio.transport.is_empty() {
            transport_icon(&radio.transport)
        } else {
            status_icon(&radio.state)
        };
        return rsx! {
            div { class: "inline-flex items-center gap-2",
                div { class: "inline-flex items-center gap-3 rounded-md border border-border bg-card px-3 py-1.5 text-xs text-muted-foreground",
                    span { class: "inline-flex min-w-0 items-center gap-2",
                        Icon { name: live_icon, size: 14 }
                        span { class: "text-foreground", "{status_label(&radio.state)}" }
                        if !label.is_empty() {
                            span { class: "max-w-[9rem] truncate text-muted-foreground", "· {label}" }
                        }
                        if radio.transport == "usb" {
                            span { class: "text-muted-foreground", "USB" }
                        } else if radio.transport == "ble" {
                            span { class: "text-muted-foreground", "BLE" }
                        } else if radio.transport == "tcp" {
                            span { class: "text-muted-foreground", "TCP" }
                        }
                    }
                    if linked || radio.battery_pct.is_some() {
                        span { class: "h-4 w-px bg-border" }
                        BatteryPill {
                            pct: radio.battery_pct,
                            mv: radio.battery_mv,
                            charging: radio.battery_charging,
                        }
                    }
                    if radio.others_connected > 0 {
                        span { class: "h-4 w-px bg-border" }
                        span { class: "text-muted-foreground", "+{radio.others_connected}" }
                    }
                }
                Button {
                    kind: ButtonKind::Ghost,
                    disabled: matches!(radio.state.as_str(), "pairing" | "disconnecting"),
                    onclick: move |_| state().radio.disconnect(),
                    Icon { name: IconName::Unlink, size: 14 }
                    if radio.state == "disconnecting" {
                        "Disconnecting…"
                    } else if busy {
                        "Cancel"
                    } else {
                        "Disconnect"
                    }
                }
            }
        };
    }

    rsx! {
        div { class: "inline-flex items-center gap-2",
            if saved_radios.is_empty() {
                div { class: "inline-flex items-center gap-2 rounded-md border border-border bg-card px-3 py-1.5 text-xs text-muted-foreground",
                    Icon { name: IconName::Radio, size: 14 }
                    span { "No saved radio" }
                }
                Button {
                    kind: ButtonKind::Ghost,
                    disabled: true,
                    onclick: move |_| {},
                    Icon { name: IconName::Radio, size: 14 }
                    "Connect"
                }
            } else {
                if saved_radios.len() > 1 {
                    div { class: "w-44 shrink-0",
                        SelectInput {
                            id: "header-saved-radio".to_string(),
                            value: selected(),
                            onchange: move |event: FormEvent| selected.set(event.value()),
                            for row in saved_radios.iter().cloned() {
                                option {
                                    key: "{row.address}",
                                    value: "{row.address}",
                                    style: "background-color: #101418; color: #f4f4f5;",
                                    "{companion_option_label(&row, &radio.devices)}"
                                }
                            }
                        }
                    }
                } else if let Some(row) = saved_radios.first() {
                    div { class: "inline-flex max-w-[12rem] items-center gap-2 rounded-md border border-border bg-card px-3 py-1.5 text-xs text-muted-foreground",
                        Icon { name: transport_icon(&row.transport), size: 14 }
                        span { class: "truncate text-foreground",
                            "{companion_display_name(row, &radio.devices)}"
                        }
                    }
                }
                span {
                    class: if busy || selected().trim().is_empty() {
                        "inline-flex"
                    } else {
                        "header-connect-pulse"
                    },
                    Button {
                        disabled: busy || selected().trim().is_empty(),
                        onclick: move |_| {
                            let target = if saved_radios.len() == 1 {
                                saved_radios
                                    .first()
                                    .map(|row| row.address.clone())
                                    .unwrap_or_else(|| selected())
                            } else {
                                selected()
                            };
                            if !target.trim().is_empty() {
                                state().radio.reconnect(target);
                            }
                        },
                        Icon { name: connect_icon, size: 14 }
                        "Connect"
                    }
                }
            }
        }
    }
}

#[component]
fn DashboardPage(radio: RadioStatus, on_navigate: EventHandler<Page>) -> Element {
    let mut overview_tab = use_signal(|| "hq".to_string());
    let hq_name = state()
        .db
        .setting("hq_name")
        .ok()
        .flatten()
        .unwrap_or_else(|| "Metis HQ".into());
    let saved_radios = state().db.list_saved_radios().unwrap_or_default();
    let preferred = state()
        .db
        .setting("radio_name")
        .ok()
        .flatten()
        .unwrap_or_default();
    let default_target = if !radio.connected_name.is_empty() {
        radio.connected_name.clone()
    } else if saved_radios.iter().any(|row| row.address == preferred) {
        preferred
    } else {
        saved_radios
            .first()
            .map(|row| row.address.clone())
            .unwrap_or_default()
    };
    let mut selected_radio = use_signal(|| default_target.clone());
    let nodes = state().db.list_nodes().unwrap_or_default();
    let pending = state().db.pending_nodes().unwrap_or_default();
    let pending_preview: Vec<_> = pending.iter().take(4).cloned().collect();
    let pending_count = pending.len();
    let recent = state().db.recent_messages(8).unwrap_or_default();
    let enrolled = nodes
        .iter()
        .filter(|node| node.enrollment == Enrollment::Enrolled)
        .count();
    let heard = nodes
        .iter()
        .filter(|node| node.enrollment == Enrollment::Heard)
        .count();
    let with_gps = nodes.iter().filter(|node| node.has_gps).count();
    let total = nodes.len();
    let radio_label = if radio.connected_name.is_empty() {
        "No companion linked".to_string()
    } else if let Some(row) = saved_radios
        .iter()
        .find(|row| {
            row.address.eq_ignore_ascii_case(&radio.connected_name)
                || row.label.eq_ignore_ascii_case(&radio.connected_name)
        })
    {
        companion_display_name(row, &radio.devices)
    } else {
        radio.connected_name.clone()
    };
    let coverage = if total == 0 {
        0
    } else {
        ((with_gps as f64 / total as f64) * 100.0).round() as i64
    };
    let busy = matches!(
        radio.state.as_str(),
        "connecting" | "reconnecting" | "pairing" | "scanning" | "scanning_usb" | "disconnecting"
    );
    let linked = radio.state == "connected";
    let stored_device = if !radio.connected_name.is_empty() {
        state()
            .db
            .device_info_for_address(&radio.connected_name)
            .ok()
            .flatten()
    } else if !selected_radio().is_empty() {
        state()
            .db
            .device_info_for_address(&selected_radio())
            .ok()
            .flatten()
    } else {
        None
    };
    let firmware = if !radio.firmware.is_empty() {
        radio.firmware.clone()
    } else {
        stored_device
            .as_ref()
            .and_then(|value| value.get("firmware"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string()
    };
    let model = if !radio.model.is_empty() {
        radio.model.clone()
    } else {
        stored_device
            .as_ref()
            .and_then(|value| value.get("model"))
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string()
    };
    let hq_raw = state().db.hq_radio_raw().ok().flatten();
    // SelfInfo stores freq/bw as milli-units (MHz×1000 / kHz×1000).
    let radio_freq = radio
        .radio_freq_mhz
        .map(|mhz| format!("{mhz:.3} MHz"))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            hq_raw.as_ref().and_then(|raw| {
                raw.get("freq").and_then(|v| v.as_f64()).map(|n| {
                    let mhz = if n > 3000.0 { n / 1000.0 } else { n };
                    format!("{mhz:.3} MHz")
                })
            })
        })
        .unwrap_or_default();
    let radio_bw = radio
        .radio_bw_khz
        .map(|khz| format!("{khz} kHz"))
        .or_else(|| {
            hq_raw.as_ref().and_then(|raw| {
                raw.get("bw").and_then(|v| v.as_f64()).map(|n| {
                    let khz = if n > 1000.0 { n / 1000.0 } else { n };
                    format!("{khz} kHz")
                })
            })
        })
        .unwrap_or_default();
    let radio_sf = radio
        .radio_sf
        .map(|sf| format!("SF{sf}"))
        .unwrap_or_else(|| json_int_label(hq_raw.as_ref(), "sf", "SF"));
    let radio_tx = radio
        .tx_power
        .map(|tx| format!("{tx} dBm"))
        .unwrap_or_else(|| json_int_label(hq_raw.as_ref(), "tx_power", "dBm"));
    let since_24h = unix_now() - 24 * 3600;
    let traffic_hourly = state().db.traffic_hourly(24).unwrap_or_default();
    let traffic_rx: Vec<f32> = traffic_hourly.iter().map(|row| row.rx as f32).collect();
    let traffic_tx: Vec<f32> = traffic_hourly.iter().map(|row| row.tx as f32).collect();
    let total_rx_24h: i64 = traffic_hourly.iter().map(|row| row.rx).sum();
    let total_tx_24h: i64 = traffic_hourly.iter().map(|row| row.tx).sum();
    let senders_hourly = state()
        .db
        .active_senders_hourly(24)
        .unwrap_or_default();
    let direct_hourly = state()
        .db
        .inbound_direct_hourly(24)
        .unwrap_or_default();
    let position_hourly = state()
        .db
        .position_updates_hourly(24)
        .unwrap_or_default();
    let messages_hourly = state().db.messages_hourly(24).unwrap_or_default();
    let messages_24h = state()
        .db
        .messages_since_count(since_24h)
        .unwrap_or(0);
    let kind_counts = state()
        .db
        .traffic_kind_counts(since_24h)
        .unwrap_or_default();
    let kind_rows = zero_filled_kind_rows(&kind_counts);
    let activity_labels = activity_hour_labels(&traffic_hourly);
    let activity_point_labels = activity_hour_point_labels(&traffic_hourly);
    let battery_points = state()
        .db
        .telemetry_series("battery", since_24h)
        .unwrap_or_default();
    let battery_chart: Vec<BatterySample> = downsample_telemetry(&battery_points, 48)
        .into_iter()
        .map(|point| BatterySample {
            at: point.at,
            pct: point.value_pct as f32,
            mv: point.value_mv as f32,
            axis_label: format_clock(point.at),
            tip_label: format_time(Some(point.at)),
        })
        .collect();
    let uptime = format_uptime(radio.connected_since);
    let hq_gps = match (radio.hq_lat, radio.hq_lon) {
        (Some(lat), Some(lon)) => Some(format!("{lat:.3}, {lon:.3}")),
        _ => state()
            .db
            .hq_position()
            .ok()
            .flatten()
            .map(|(_, lat, lon)| format!("{lat:.3}, {lon:.3}")),
    };
    let companion_shell = if linked {
        "rounded-lg border border-signal/35 bg-card p-4"
    } else {
        "rounded-lg border border-border bg-card p-4"
    };
    let battery_pct = radio.battery_pct.unwrap_or(0);
    let battery_width = if linked { battery_pct } else { 0 };
    let selected_transport = saved_radios
        .iter()
        .find(|row| row.address == selected_radio())
        .map(|row| row.transport.as_str())
        .unwrap_or("");
    let companion_transport = if !radio.transport.is_empty() {
        radio.transport.as_str()
    } else {
        selected_transport
    };
    let companion_icon = transport_icon(companion_transport);
    let companion_transport_chip = transport_label(companion_transport);
    let pending_alerts = state()
        .db
        .list_pending_service_events()
        .unwrap_or_default()
        .len();
    let primary_id = state().db.primary_radio_id().ok().flatten();
    let primary_row = primary_id.and_then(|id| saved_radios.iter().find(|row| row.id == id));
    let pool_live = if linked {
        1u32 + radio.others_connected
    } else {
        radio.others_connected
    };
    let pool_hint = if saved_radios.is_empty() {
        "No companions saved".to_string()
    } else if let Some(row) = primary_row {
        format!(
            "{} primary · {} live",
            companion_display_name(row, &radio.devices),
            pool_live
        )
    } else {
        format!(
            "{} saved · {} live · no primary",
            saved_radios.len(),
            pool_live
        )
    };
    let latest_backup = crate::backup::list_backup_entries(&crate::data_dir())
        .into_iter()
        .next();
    let backup_hint = match latest_backup.as_ref() {
        Some(entry) if entry.created_at > 0 => format_age_secs(entry.created_at),
        Some(_) => "Backup on disk".to_string(),
        None => "No backup yet".to_string(),
    };
    let fw_sync = crate::firmware::sync_status();
    let firmware_hint = if fw_sync.running {
        if fw_sync.phase.is_empty() {
            "Syncing…".to_string()
        } else {
            format!("Syncing · {}", fw_sync.phase)
        }
    } else if fw_sync.last_ok_at > 0 {
        format!(
            "{} · {}",
            format_age_secs(fw_sync.last_ok_at),
            crate::firmware::format_bytes(fw_sync.bytes_on_disk)
        )
    } else if fw_sync.asset_count > 0 {
        format!(
            "{} assets · {}",
            fw_sync.asset_count,
            crate::firmware::format_bytes(fw_sync.bytes_on_disk)
        )
    } else {
        "Locker empty".to_string()
    };
    let alerts_hint = if pending_alerts == 0 {
        "No pending alerts".to_string()
    } else if pending_alerts == 1 {
        "1 alert waiting".to_string()
    } else {
        format!("{pending_alerts} alerts waiting")
    };
    let mention_unread = state()
        .db
        .total_unread_mentions()
        .unwrap_or(0)
        .max(0);
    let channel_total = state().db.count_messages_kind("channel").unwrap_or(0);
    let direct_total = state().db.count_messages_kind("direct").unwrap_or(0);
    let dm_unread = state().db.total_dm_unread().unwrap_or(0).max(0);
    let unread_total = state().db.total_channel_unread().unwrap_or(0).max(0) + dm_unread;
    let mentions_hint = if mention_unread > 0 {
        if mention_unread == 1 {
            "1 unread @mention".to_string()
        } else {
            format!("{mention_unread} unread @mentions")
        }
    } else {
        "No unread @mentions".to_string()
    };
    let unread_hint = if unread_total > 0 {
        if unread_total == 1 {
            "1 unread message".to_string()
        } else {
            format!("{unread_total} unread messages")
        }
    } else {
        "No unread messages".to_string()
    };
    let channels_hint = if channel_total == 1 {
        "1 channel message".to_string()
    } else {
        format!("{channel_total} channel messages")
    };
    let direct_hint = if direct_total == 1 {
        "1 direct message".to_string()
    } else {
        format!("{direct_total} direct messages")
    };

    rsx! {
        div {
            class: "mx-auto flex w-full max-w-7xl flex-col gap-4 rounded-2xl p-4 sm:p-5",
            style: "background: linear-gradient(180deg, #0c0f12 0%, #12171c 100%);",
            PageHeading { icon: IconName::Dashboard, title: "Overview".to_string(),
                "Live status for the headquarters radio, roster, and recent mesh traffic."
            }
            {
                let show_repeaters_tab =
                    state().db.count_claimed_infra().unwrap_or(0) > 0;
                let on_repeaters =
                    show_repeaters_tab && overview_tab() == "repeaters";
                rsx! {
                    if show_repeaters_tab {
                        div { class: "settings-subtabs",
                            button {
                                class: if !on_repeaters {
                                    "settings-subtab settings-subtab-active"
                                } else {
                                    "settings-subtab"
                                },
                                onclick: move |_| overview_tab.set("hq".into()),
                                "HQ"
                            }
                            button {
                                class: if on_repeaters {
                                    "settings-subtab settings-subtab-active"
                                } else {
                                    "settings-subtab"
                                },
                                onclick: move |_| overview_tab.set("repeaters".into()),
                                "Repeaters"
                            }
                        }
                    }
                    if on_repeaters {
                        overview_repeaters::OverviewRepeatersTab { on_navigate }
                    }
                }
            }
            if !(state().db.count_claimed_infra().unwrap_or(0) > 0
                && overview_tab() == "repeaters")
            {
            div { class: "overview-chip-stack",
                section { class: "overview-chip-row", aria_label: "System status",
                    button {
                        class: "overview-chip overview-chip-alerts",
                        onclick: move |_| on_navigate.call(Page::Inbox),
                        span { class: "overview-chip__icon",
                            Icon { name: IconName::Inbox, size: 14 }
                        }
                        span { class: "min-w-0",
                            span { class: "block text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Alerts" }
                            span { class: "block truncate text-sm text-foreground", "{alerts_hint}" }
                        }
                    }
                    button {
                        class: "overview-chip overview-chip-radio",
                        onclick: move |_| on_navigate.call(Page::Settings),
                        span { class: "overview-chip__icon",
                            Icon { name: IconName::Radio, size: 14 }
                        }
                        span { class: "min-w-0",
                            span { class: "block text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Radio pool" }
                            span { class: "block truncate text-sm text-foreground", "{pool_hint}" }
                        }
                    }
                    button {
                        class: "overview-chip overview-chip-backup",
                        onclick: move |_| on_navigate.call(Page::Settings),
                        span { class: "overview-chip__icon",
                            Icon { name: IconName::Key, size: 14 }
                        }
                        span { class: "min-w-0",
                            span { class: "block text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Backup" }
                            span { class: "block truncate text-sm text-foreground", "{backup_hint}" }
                        }
                    }
                    button {
                        class: "overview-chip overview-chip-firmware",
                        onclick: move |_| on_navigate.call(Page::Settings),
                        span { class: "overview-chip__icon",
                            Icon { name: IconName::Scan, size: 14 }
                        }
                        span { class: "min-w-0",
                            span { class: "block text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Firmware" }
                            span { class: "block truncate text-sm text-foreground", "{firmware_hint}" }
                        }
                    }
                }
                section { class: "overview-chip-row", aria_label: "Messages",
                    button {
                        class: if unread_total > 0 {
                            "overview-chip overview-chip-unread overview-chip-unread-live"
                        } else {
                            "overview-chip overview-chip-unread"
                        },
                        onclick: move |_| on_navigate.call(Page::Messages),
                        span { class: "overview-chip__icon",
                            Icon { name: IconName::Inbox, size: 14 }
                        }
                        span { class: "min-w-0",
                            span { class: "block text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Unread" }
                            span {
                                class: if unread_total > 0 {
                                    "block truncate text-sm overview-chip-unread__value"
                                } else {
                                    "block truncate text-sm text-foreground"
                                },
                                "{unread_hint}"
                            }
                        }
                    }
                    button {
                        class: if mention_unread > 0 {
                            "overview-chip overview-chip-mentions overview-chip-mentions-live"
                        } else {
                            "overview-chip overview-chip-mentions"
                        },
                        onclick: move |_| on_navigate.call(Page::Messages),
                        span { class: "overview-chip__icon",
                            Icon { name: IconName::Messages, size: 14 }
                        }
                        span { class: "min-w-0",
                            span { class: "block text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Mentions" }
                            span {
                                class: if mention_unread > 0 {
                                    "block truncate text-sm text-amber-200"
                                } else {
                                    "block truncate text-sm text-foreground"
                                },
                                "{mentions_hint}"
                            }
                        }
                    }
                    button {
                        class: "overview-chip overview-chip-channels",
                        onclick: move |_| on_navigate.call(Page::Messages),
                        span { class: "overview-chip__icon",
                            Icon { name: IconName::Messages, size: 14 }
                        }
                        span { class: "min-w-0",
                            span { class: "block text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Channels" }
                            span { class: "block truncate text-sm text-foreground", "{channels_hint}" }
                        }
                    }
                    button {
                        class: if dm_unread > 0 {
                            "overview-chip overview-chip-direct overview-chip-direct-live"
                        } else {
                            "overview-chip overview-chip-direct"
                        },
                        onclick: move |_| on_navigate.call(Page::Messages),
                        span { class: "overview-chip__icon",
                            Icon { name: IconName::Inbox, size: 14 }
                        }
                        span { class: "min-w-0",
                            span { class: "block text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Direct" }
                            span {
                                class: if dm_unread > 0 {
                                    "block truncate text-sm overview-chip-direct__value"
                                } else {
                                    "block truncate text-sm text-foreground"
                                },
                                "{direct_hint}"
                            }
                        }
                    }
                }
            }
            div { class: "grid grid-cols-2 gap-3 lg:grid-cols-4",
                OverviewStat {
                    icon: IconName::Users,
                    label: "Enrolled".to_string(),
                    value: enrolled.to_string(),
                    hint: format!("{heard} heard, {total} known"),
                    tone: "signal".to_string(),
                    series: senders_hourly.clone(),
                    labels: activity_point_labels.clone(),
                    value_suffix: "senders".to_string(),
                    delta: last_hour_delta(&senders_hourly),
                    onclick: move |_| on_navigate.call(Page::Roster),
                }
                OverviewStat {
                    icon: IconName::Inbox,
                    label: "Pending signup".to_string(),
                    value: pending_count.to_string(),
                    hint: if pending_count == 0 {
                        "Inbox is clear".to_string()
                    } else {
                        "Waiting for operator approval".to_string()
                    },
                    tone: if pending_count == 0 {
                        "muted".to_string()
                    } else {
                        "pending".to_string()
                    },
                    series: direct_hourly.clone(),
                    labels: activity_point_labels.clone(),
                    value_suffix: "DMs".to_string(),
                    delta: last_hour_delta(&direct_hourly),
                    onclick: move |_| on_navigate.call(Page::Inbox),
                }
                OverviewStat {
                    icon: IconName::Locate,
                    label: "GPS coverage".to_string(),
                    value: format!("{coverage}%"),
                    hint: format!("{with_gps} of {total} nodes with a fix"),
                    tone: "ok".to_string(),
                    series: position_hourly.clone(),
                    labels: activity_point_labels.clone(),
                    value_suffix: "fixes".to_string(),
                    delta: last_hour_delta(&position_hourly),
                    onclick: move |_| on_navigate.call(Page::Map),
                }
                OverviewStat {
                    icon: IconName::Messages,
                    label: "Recent traffic".to_string(),
                    value: messages_24h.to_string(),
                    hint: "Messages in the last 24h".to_string(),
                    tone: "muted".to_string(),
                    series: messages_hourly.clone(),
                    labels: activity_point_labels.clone(),
                    value_suffix: "msgs".to_string(),
                    delta: last_hour_delta(&messages_hourly),
                    onclick: move |_| on_navigate.call(Page::Messages),
                }
            }
            div {
                class: "overview-hero-grid",
                style: "display: grid; gap: 0.75rem;",
                section { class: "{companion_shell}",
                    div { class: "mb-3 flex flex-wrap items-center justify-between gap-2",
                        if linked {
                            span { class: "status-pill-online",
                                span { class: "h-1.5 w-1.5 rounded-full bg-[#101418]" }
                                "Online"
                            }
                        } else {
                            span { class: "status-pill-offline",
                                span { class: "h-1.5 w-1.5 rounded-full bg-amber-300" }
                                "Offline"
                            }
                        }
                        div { class: "inline-flex flex-wrap items-center gap-1.5",
                            if companion_transport == "usb"
                                || companion_transport == "ble"
                                || companion_transport == "tcp"
                            {
                                span { class: "spec-chip",
                                    Icon { name: companion_icon, size: 12 }
                                    span { class: "ml-1", "{companion_transport_chip}" }
                                }
                            }
                            span { class: "spec-chip",
                                Icon { name: IconName::Radio, size: 12 }
                                span { class: "ml-1", "HQ" }
                            }
                        }
                    }
                    div { class: "flex items-start gap-3",
                        div { class: "flex h-11 w-11 shrink-0 items-center justify-center rounded-md bg-accent text-signal",
                            Icon { name: companion_icon, size: 22 }
                        }
                        div { class: "min-w-0 flex-1",
                            p { class: "truncate text-xl font-semibold tracking-tight", "{radio_label}" }
                            if linked && !radio.connected_name.is_empty()
                                && !radio_label.eq_ignore_ascii_case(&radio.connected_name)
                            {
                                p { class: "mt-0.5 truncate font-mono text-[0.7rem] text-muted-foreground", "{radio.connected_name}" }
                            }
                            p { class: "mt-1 text-xs text-muted-foreground",
                                if radio.sync_phase.is_active() {
                                    "{radio.sync_label}"
                                } else if linked {
                                    if companion_transport == "usb" {
                                        "USB companion linked · synced"
                                    } else if companion_transport == "ble" {
                                        "Bluetooth companion linked · synced"
                                    } else if companion_transport == "tcp" {
                                        "Network companion linked · synced"
                                    } else {
                                        "Companion linked · synced"
                                    }
                                } else {
                                    "Pick a saved companion and connect"
                                }
                            }
                        }
                    }
                    div { class: "mt-4 rounded-md border border-border bg-[#12181f] p-3",
                        div { class: "mb-2 flex flex-wrap items-center justify-between gap-2",
                            div { class: "inline-flex items-center gap-2",
                                BatteryPill {
                                    pct: radio.battery_pct,
                                    mv: radio.battery_mv,
                                    charging: radio.battery_charging,
                                }
                                span { class: "text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Battery" }
                            }
                            div { class: "inline-flex items-center gap-2 text-xs text-muted-foreground",
                                if let Some(mv) = radio.battery_mv {
                                    span { class: "tabular-nums", "{mv} mV" }
                                }
                                if radio.battery_charging {
                                    span { class: "text-signal", "Charging" }
                                }
                            }
                        }
                        div { class: "battery-liquid",
                            span { style: "width: {battery_width}%;" }
                        }
                        if !battery_chart.is_empty() {
                            div { class: "mt-2.5 border-t border-border pt-2",
                                p { class: "mb-1 text-[0.65rem] text-muted-foreground",
                                    "Battery voltage · last 24 hours"
                                }
                                BatteryChart { points: battery_chart.clone() }
                            }
                        }
                    }
                    div { class: "mt-3 grid grid-cols-3 gap-2",
                        div { class: "metric-tile",
                            p { class: "text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Uptime" }
                            p { class: "mt-1 text-sm font-semibold tabular-nums",
                                if let Some(uptime) = uptime.clone() { "{uptime}" } else { "—" }
                            }
                        }
                        div { class: "metric-tile",
                            p { class: "text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "HQ GPS" }
                            p { class: "mt-1 truncate font-mono text-xs",
                                if let Some(coords) = hq_gps.clone() { "{coords}" } else { "—" }
                            }
                        }
                        div { class: "metric-tile",
                            p { class: "text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Nodes" }
                            p { class: "mt-1 text-sm font-semibold tabular-nums", "{total}" }
                        }
                    }
                    div { class: "mt-3 flex flex-wrap gap-1.5",
                        if !firmware.is_empty() {
                            span { class: "spec-chip", "{firmware}" }
                        }
                        if !model.is_empty() {
                            span { class: "spec-chip", "{model}" }
                        }
                        if !radio_freq.is_empty() {
                            span { class: "spec-chip", "{radio_freq}" }
                        }
                        if !radio_bw.is_empty() {
                            span { class: "spec-chip", "{radio_bw}" }
                        }
                        if !radio_sf.is_empty() {
                            span { class: "spec-chip", "{radio_sf}" }
                        }
                        if !radio_tx.is_empty() {
                            span { class: "spec-chip", "{radio_tx}" }
                        }
                    }
                    div { class: "mt-3 flex flex-col gap-2 border-t border-border pt-3",
                        if saved_radios.is_empty() {
                            p { class: "text-xs text-muted-foreground",
                                "No saved companions yet. Add one from Settings."
                            }
                        } else {
                            div { class: "flex items-end gap-2",
                                div { class: "min-w-0 flex-1",
                                    label {
                                        class: "mb-1 block text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground",
                                        r#for: "overview-saved-radio",
                                        "Saved companion"
                                    }
                                    SelectInput {
                                        id: "overview-saved-radio".to_string(),
                                        value: selected_radio(),
                                        onchange: move |event: FormEvent| selected_radio.set(event.value()),
                                        for row in saved_radios.iter().cloned() {
                                            option {
                                                key: "{row.address}",
                                                value: "{row.address}",
                                                style: "background-color: #101418; color: #f4f4f5;",
                                                "{companion_option_label(&row, &radio.devices)}"
                                            }
                                        }
                                    }
                                }
                                if linked {
                                    Button {
                                        kind: ButtonKind::Ghost,
                                        onclick: move |_| state().radio.disconnect(),
                                        Icon { name: IconName::Unlink, size: 14 }
                                        "Disconnect"
                                    }
                                } else {
                                    Button {
                                        disabled: busy || selected_radio().trim().is_empty(),
                                        onclick: move |_| state().radio.reconnect(selected_radio()),
                                        Icon { name: companion_icon, size: 14 }
                                        if busy { "Working" } else { "Connect" }
                                    }
                                }
                            }
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| on_navigate.call(Page::Settings),
                            Icon { name: IconName::Settings, size: 14 }
                            "Add companion"
                        }
                    }
                }
                div { class: "flex flex-col gap-3",
                    section { class: "rounded-lg border border-border bg-card px-4 py-3",
                        div { class: "flex flex-wrap items-center justify-between gap-3",
                            div { class: "flex min-w-0 items-center gap-3",
                                div { class: "flex h-9 w-9 shrink-0 items-center justify-center rounded-md bg-accent text-signal",
                                    Icon { name: IconName::Radio, size: 18 }
                                }
                                div { class: "min-w-0",
                                    p { class: "truncate font-semibold", "{hq_name}" }
                                    p { class: "text-xs text-muted-foreground",
                                        if linked {
                                            "On air through the companion"
                                        } else {
                                            "Standby · local SQLite"
                                        }
                                    }
                                }
                            }
                            div { class: "flex flex-wrap gap-2",
                                Badge {
                                    tone: if linked { "enrolled".to_string() } else { "pending".to_string() },
                                    if linked { "On air" } else { "Standby" }
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| on_navigate.call(Page::Settings),
                                    Icon { name: IconName::Settings, size: 14 }
                                    "HQ"
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| on_navigate.call(Page::Map),
                                    Icon { name: IconName::Map, size: 14 }
                                    "Map"
                                }
                            }
                        }
                    }
                    section { class: "rounded-lg border border-border bg-card p-4",
                        div { class: "mb-3 flex items-center gap-2",
                            Icon { name: IconName::Messages, size: 16 }
                            div {
                                p { class: "text-sm font-semibold tracking-wide", "Mesh traffic" }
                                p { class: "text-[0.7rem] text-muted-foreground", "Messages through HQ in the last 24 hours" }
                            }
                        }
                        div {
                            style: "display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:0.65rem;",
                            div { class: "metric-tile",
                                div { class: "mb-1 flex items-center gap-1.5 text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground",
                                    Icon { name: IconName::Signal, size: 12 }
                                    "Sent · TX"
                                }
                                p { class: "text-xl font-semibold tabular-nums text-signal", "{total_tx_24h}" }
                                p { class: "mt-0.5 text-[0.65rem] text-muted-foreground", "Outbound last 24h" }
                                Sparkline {
                                    values: traffic_tx.clone(),
                                    tone: "signal".to_string(),
                                    labels: activity_point_labels.clone(),
                                    value_suffix: "msgs".to_string(),
                                }
                            }
                            div { class: "metric-tile",
                                div { class: "mb-1 flex items-center gap-1.5 text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground",
                                    Icon { name: IconName::Inbox, size: 12 }
                                    "Received · RX"
                                }
                                p { class: "text-xl font-semibold tabular-nums text-sky-200", "{total_rx_24h}" }
                                p { class: "mt-0.5 text-[0.65rem] text-muted-foreground", "Inbound last 24h" }
                                Sparkline {
                                    values: traffic_rx.clone(),
                                    tone: "ok".to_string(),
                                    labels: activity_point_labels.clone(),
                                    value_suffix: "msgs".to_string(),
                                }
                            }
                        }
                    }
                    if pending_count == 0 {
                        button {
                            class: "flex w-full items-center justify-between rounded-lg border border-border bg-card px-4 py-3 text-left hover:bg-muted/40 cursor-pointer",
                            onclick: move |_| on_navigate.call(Page::Inbox),
                            span { class: "inline-flex items-center gap-2 text-sm text-muted-foreground",
                                Icon { name: IconName::Inbox, size: 16 }
                                "Inbox clear · no pending signups"
                            }
                            Icon { name: IconName::ChevronRight, size: 16 }
                        }
                    } else {
                        Card { title: "Signup queue".to_string(), icon: Some(IconName::Inbox),
                            div { class: "flex flex-col gap-2",
                                for node in pending_preview {
                                    button {
                                        class: "flex w-full items-center justify-between rounded-md border border-border bg-background px-3 py-2 text-left hover:bg-muted cursor-pointer",
                                        onclick: move |_| on_navigate.call(Page::Inbox),
                                        div { class: "min-w-0",
                                            p { class: "truncate text-sm font-medium", "{node_title(&node.name, &node.requested_name)}" }
                                            p { class: "font-mono text-xs text-muted-foreground", "{node.prefix}" }
                                        }
                                        Icon { name: IconName::ChevronRight, size: 16 }
                                    }
                                }
                            }
                            div { class: "mt-4",
                                Button {
                                    onclick: move |_| on_navigate.call(Page::Inbox),
                                    Icon { name: IconName::Inbox, size: 14 }
                                    "Review inbox"
                                }
                            }
                        }
                    }
                }
            }
            div {
                class: "overview-charts-grid",
                style: "display: grid; gap: 0.75rem;",
                Card { title: "Mesh activity (24h)".to_string(), icon: Some(IconName::Signal),
                    p { class: "mb-3 text-xs text-muted-foreground",
                        "RX {total_rx_24h} · TX {total_tx_24h} over the last day"
                    }
                    LineChart {
                        series: vec![
                            ChartSeries {
                                label: "Received".into(),
                                tone: "rx".into(),
                                values: traffic_rx,
                            },
                            ChartSeries {
                                label: "Transmitted".into(),
                                tone: "tx".into(),
                                values: traffic_tx,
                            },
                        ],
                        height: 160.0,
                        labels: activity_labels,
                        point_labels: activity_point_labels,
                    }
                }
                Card { title: "Traffic mix".to_string(), icon: Some(IconName::Messages),
                    p { class: "mb-3 text-xs text-muted-foreground",
                        "Message kinds stored in the last 24 hours"
                    }
                    BarMix { rows: kind_rows }
                }
            }
            Card { title: "Latest traffic".to_string(), icon: Some(IconName::Messages),
                if recent.is_empty() {
                    p { class: "text-sm text-muted-foreground", "No messages stored yet." }
                } else {
                    div { class: "overflow-hidden rounded-md border border-border",
                        table { class: "w-full text-left text-sm",
                            thead { class: "bg-background text-xs uppercase tracking-wide text-muted-foreground",
                                tr {
                                    th { class: "px-3 py-2 font-medium", "When" }
                                    th { class: "px-3 py-2 font-medium", "From" }
                                    th { class: "px-3 py-2 font-medium", "Hops" }
                                    th { class: "px-3 py-2 font-medium", "Text" }
                                }
                            }
                            tbody {
                                for message in recent {
                                    tr { key: "{message.id}", class: "border-t border-border align-top",
                                        td { class: "px-3 py-2 whitespace-nowrap text-muted-foreground", "{format_time(Some(message.received_at))}" }
                                        td { class: "px-3 py-2", "{message.sender_name}" }
                                        td { class: "px-3 py-2",
                                            if let Some(hops) = message.hop_count { "{hops}" } else { "—" }
                                        }
                                        td { class: "px-3 py-2",
                                            MessageBody {
                                                text: message.text.clone(),
                                                outgoing: message.direction == "tx",
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "mt-4",
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| on_navigate.call(Page::Messages),
                        Icon { name: IconName::Messages, size: 14 }
                        "All messages"
                    }
                }
            }
            } // end overview HQ tab
        }
    }
}

fn downsample_telemetry(points: &[TelemetryPoint], target: usize) -> Vec<TelemetryPoint> {
    if points.is_empty() || target == 0 {
        return Vec::new();
    }
    if points.len() <= target {
        return points.to_vec();
    }
    let chunk = points.len() as f32 / target as f32;
    (0..target)
        .map(|index| {
            let start = (index as f32 * chunk).floor() as usize;
            let end = (((index as f32 + 1.0) * chunk).floor() as usize)
                .max(start + 1)
                .min(points.len());
            let slice = &points[start..end];
            let n = slice.len() as f64;
            let value_mv = slice.iter().map(|p| p.value_mv).sum::<f64>() / n;
            let value_pct = slice.iter().map(|p| p.value_pct).sum::<f64>() / n;
            let at = slice[slice.len() / 2].at;
            TelemetryPoint {
                at,
                value_pct,
                value_mv,
            }
        })
        .collect()
}

fn format_uptime(since: Option<i64>) -> Option<String> {
    let since = since?;
    let secs = (unix_now() - since).max(0);
    if secs < 60 {
        Some(format!("{secs}s"))
    } else if secs < 3600 {
        Some(format!("{}m", secs / 60))
    } else if secs < 86_400 {
        Some(format!("{}h {}m", secs / 3600, (secs % 3600) / 60))
    } else {
        Some(format!("{}d {}h", secs / 86_400, (secs % 86_400) / 3600))
    }
}

fn last_hour_delta(series: &[f32]) -> Option<i64> {
    let last = *series.last()? as i64;
    if last > 0 {
        Some(last)
    } else {
        None
    }
}

fn json_int_label(value: Option<&serde_json::Value>, key: &str, prefix: &str) -> String {
    let Some(value) = value.and_then(|row| row.get(key)) else {
        return String::new();
    };
    let number = value
        .as_i64()
        .or_else(|| value.as_f64().map(|n| n.round() as i64))
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()));
    match number {
        Some(n) if n != 0 || key == "sf" => format!("{prefix}{n}"),
        _ => String::new(),
    }
}

fn activity_hour_labels(hours: &[crate::db::HourlyTraffic]) -> Vec<String> {
    if hours.is_empty() {
        return Vec::new();
    }
    let last = hours.len() - 1;
    let idxs = [0, last / 3, (2 * last) / 3, last];
    idxs.into_iter()
        .map(|index| {
            if index == last {
                "now".into()
            } else {
                hour_tick_label(hours[index].hour_start)
            }
        })
        .collect()
}

fn activity_hour_point_labels(hours: &[crate::db::HourlyTraffic]) -> Vec<String> {
    let last = hours.len().saturating_sub(1);
    hours
        .iter()
        .enumerate()
        .map(|(index, row)| {
            if index == last {
                "now".into()
            } else {
                hour_tick_label(row.hour_start)
            }
        })
        .collect()
}

fn hour_tick_label(ts: i64) -> String {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .map(|time| {
            let (is_pm, hour) = time.hour12();
            format!("{hour}{}", if is_pm { "p" } else { "a" })
        })
        .unwrap_or_else(|| "—".into())
}

fn zero_filled_kind_rows(counts: &[crate::db::KindCount]) -> Vec<BarMixRow> {
    let lookup = |kind: &str| -> i64 {
        counts
            .iter()
            .find(|row| row.kind.eq_ignore_ascii_case(kind))
            .map(|row| row.count)
            .unwrap_or(0)
    };
    let mut rows = vec![
        BarMixRow {
            label: "Channel".into(),
            count: lookup("channel"),
            tone: "signal".into(),
        },
        BarMixRow {
            label: "Direct".into(),
            count: lookup("direct"),
            tone: "ok".into(),
        },
        BarMixRow {
            label: "Advert".into(),
            count: lookup("advert"),
            tone: "pending".into(),
        },
    ];
    for row in counts {
        if !matches!(
            row.kind.to_ascii_lowercase().as_str(),
            "channel" | "direct" | "advert"
        ) {
            rows.push(BarMixRow {
                label: row.kind.clone(),
                count: row.count,
                tone: "muted".into(),
            });
        }
    }
    rows
}

#[component]
fn OverviewStat(
    icon: IconName,
    label: String,
    value: String,
    hint: String,
    tone: String,
    #[props(default)] series: Vec<f32>,
    /// Per-hour tip labels (e.g. `2p`, `now`) — avoids `#13` index fallback.
    #[props(default)]
    labels: Vec<String>,
    #[props(default)]
    value_suffix: String,
    #[props(default)] delta: Option<i64>,
    onclick: EventHandler<MouseEvent>,
) -> Element {
    let value_class = match tone.as_str() {
        "signal" => "text-signal",
        "pending" => "text-amber-200",
        "ok" => "text-sky-200",
        _ => "text-foreground",
    };
    let shell_class = match tone.as_str() {
        "signal" => "overview-stat overview-stat-signal",
        "pending" => "overview-stat overview-stat-pending",
        "ok" => "overview-stat overview-stat-ok",
        _ => "overview-stat overview-stat-muted",
    };
    rsx! {
        button {
            class: "{shell_class}",
            onclick: move |event| onclick.call(event),
            div { class: "mb-1 flex items-center justify-between gap-2 text-muted-foreground",
                span { class: "truncate text-[0.65rem] uppercase tracking-[0.14em]", "{label}" }
                Icon { name: icon, size: 14 }
            }
            div { class: "flex items-baseline justify-between gap-2",
                p { class: "text-xl font-semibold tracking-tight {value_class}", "{value}" }
                if let Some(delta) = delta {
                    span { class: "text-[0.7rem] tabular-nums text-signal", "+{delta} /hr" }
                }
            }
            p { class: "mt-0.5 truncate text-[0.7rem] text-muted-foreground", "{hint}" }
            Sparkline {
                values: series,
                tone,
                labels,
                value_suffix,
            }
        }
    }
}

#[component]
fn RosterPage() -> Element {
    let nodes = state().db.list_nodes().unwrap_or_default();
    let private_channels: Vec<_> = state()
        .db
        .list_channels()
        .unwrap_or_default()
        .into_iter()
        .filter(|row| row.visibility == "private")
        .collect();
    let secrets = state().secrets.clone();
    let mut query = use_signal(String::new);
    let mut note = use_signal(String::new);
    let mut dm_target = use_signal(|| None::<(String, String)>);
    let mut dm_body = use_signal(String::new);
    let mut invite_target = use_signal(|| None::<(String, String)>);
    let mut claim_target = use_signal(|| None::<(String, String)>);
    let mut claim_password = use_signal(String::new);
    let mut claim_busy = use_signal(|| false);
    let mut claim_error = use_signal(String::new);
    let mut details_pubkey = use_signal(|| None::<String>);
    let mut confirm_clear_new = use_signal(|| false);
    let mut confirm_prune_heard = use_signal(|| false);
    let mut invite_channel = use_signal(|| {
        private_channels
            .first()
            .map(|row| row.name.clone())
            .unwrap_or_else(|| {
                state()
                    .db
                    .info_channel_name()
                    .unwrap_or_else(|_| crate::secrets::CHANNEL_NAME.to_string())
            })
    });
    let unnoticed_count = state().db.count_unnoticed_nodes().unwrap_or(0);
    let heard_only_count = state().db.count_heard_only_nodes().unwrap_or(0);
    let needle = query();
    let mut ranked: Vec<_> = nodes
        .into_iter()
        .filter_map(|node| {
            let title = node_title(&node.name, &node.requested_name);
            let score = fuzzy_score(
                &needle,
                &[
                    title.as_str(),
                    node.name.as_str(),
                    node.prefix.as_str(),
                    node.pubkey.as_str(),
                    node.enrollment.label(),
                    roster_type_label(node.enrollment, node.contact_type),
                    contact_type_label(node.contact_type),
                ],
            )?;
            Some((score, node, title))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase())));
    let claimed_keys = state()
        .db
        .claimed_pubkey_set()
        .unwrap_or_default();

    let details_key = details_pubkey();
    let details_node = details_key
        .as_ref()
        .and_then(|key| state().db.get_node(key).ok().flatten());
    let details_title = details_node
        .as_ref()
        .map(|n| node_title(&n.name, &n.requested_name))
        .unwrap_or_else(|| "Device details".into());
    let details_tracked = details_key
        .as_ref()
        .map(|key| {
            state()
                .db
                .tracked_pubkey_set()
                .ok()
                .map(|set| set.contains(key))
                .unwrap_or(false)
        })
        .unwrap_or(false);
    let details_rf = details_key
        .as_ref()
        .and_then(|key| state().db.rf_samples_for_pubkey(key, 5).ok())
        .unwrap_or_default();
    let details_messages = details_key
        .as_ref()
        .and_then(|key| state().db.messages_for_pubkey(key, 8).ok())
        .unwrap_or_default();
    let details_positions = details_key
        .as_ref()
        .and_then(|key| state().db.positions_for_pubkeys(&[key.clone()], None).ok())
        .unwrap_or_default();
    let details_pos_count = details_key
        .as_ref()
        .and_then(|key| state().db.position_count_for_pubkey(key).ok())
        .unwrap_or(0);
    let details_raw = details_key
        .as_ref()
        .and_then(|key| state().db.node_raw_json(key).ok().flatten())
        .unwrap_or_else(|| "{}".into());
    let details_raw_pretty = serde_json::from_str::<serde_json::Value>(&details_raw)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or(details_raw);
    let details_hops = details_node
        .as_ref()
        .map(|n| {
            if n.hop_bytes.is_empty() {
                "—".into()
            } else {
                hex::encode(&n.hop_bytes)
            }
        })
        .unwrap_or_else(|| "—".into());
    let details_open = details_node.is_some();

    rsx! {
        div { class: "flex flex-col gap-3",
            SlideDownPanel {
                open: details_open,
                title: details_title.clone(),
                subtitle: "Everything HQ has stored for this radio.".to_string(),
                icon: Some(IconName::Users),
                on_close: move |_| details_pubkey.set(None),
                if let Some(node) = details_node.as_ref() {
                    div { class: "flex flex-col gap-4 text-sm",
                        div { class: "grid gap-3 sm:grid-cols-2",
                            DetailField { label: "Enrollment".to_string(),
                                Badge { tone: node.enrollment.as_str().to_string(), "{node.enrollment.label()}" }
                            }
                            DetailField { label: "Type".to_string(),
                                ContactTypePill {
                                    enrollment: node.enrollment,
                                    contact_type: node.contact_type,
                                }
                            }
                            DetailField { label: "Prefix".to_string(),
                                span { class: "font-mono text-xs", "{node.prefix}" }
                            }
                            DetailField { label: "On Tracker".to_string(),
                                if details_tracked { "Yes" } else { "No" }
                            }
                            DetailField { label: "Last heard".to_string(), "{format_time(node.last_heard)}" }
                            DetailField { label: "Last advert".to_string(), "{format_time(node.last_advert)}" }
                            {
                                let last_via = node
                                    .last_radio_id
                                    .and_then(|id| state().db.radio_label(id).ok().flatten())
                                    .unwrap_or_default();
                                let also = details_key
                                    .as_ref()
                                    .and_then(|pk| state().db.list_node_radios(pk).ok())
                                    .unwrap_or_default()
                                    .into_iter()
                                    .filter(|(id, _)| Some(*id) != node.last_radio_id)
                                    .filter_map(|(id, _)| state().db.radio_label(id).ok().flatten())
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                rsx! {
                                    if !last_via.is_empty() {
                                        DetailField { label: "Last via".to_string(), "{last_via}" }
                                    }
                                    if !also.is_empty() {
                                        DetailField { label: "Also heard on".to_string(), "{also}" }
                                    }
                                }
                            }
                            DetailField { label: "Path length".to_string(),
                                if node.path_len >= 0 { "{node.path_len}" } else { "—" }
                            }
                            DetailField { label: "Hop hashes".to_string(),
                                span { class: "break-all font-mono text-xs text-muted-foreground", "{details_hops}" }
                            }
                        }
                        div { class: "rounded-md border border-border bg-background/60 p-3",
                            p { class: "mb-2 text-xs uppercase tracking-[0.16em] text-muted-foreground", "Identity" }
                            p { class: "break-all font-mono text-xs leading-relaxed", "{node.pubkey}" }
                            if let Some(req) = node.requested_name.as_ref() {
                                if !req.is_empty() {
                                    p { class: "mt-2 text-xs text-muted-foreground", "Requested name: {req}" }
                                }
                            }
                        }
                        div { class: "rounded-md border border-border bg-background/60 p-3",
                            p { class: "mb-2 text-xs uppercase tracking-[0.16em] text-muted-foreground", "Location" }
                            if let (Some(lat), Some(lon)) = (node.lat, node.lon) {
                                p { class: "font-mono text-xs", "{lat:.6}, {lon:.6}" }
                                p { class: "mt-1 text-xs text-muted-foreground",
                                    "{details_pos_count} position sample(s) in trail history"
                                }
                                if !details_positions.is_empty() {
                                    ul { class: "mt-2 max-h-28 space-y-1 overflow-y-auto text-xs text-muted-foreground",
                                        for point in details_positions.iter().rev().take(8) {
                                            li { class: "font-mono",
                                                "{point.lat:.5}, {point.lon:.5}"
                                            }
                                        }
                                    }
                                }
                            } else {
                                p { class: "text-muted-foreground", "No GPS fix stored (or 0,0 ignored)." }
                            }
                        }
                        div { class: "rounded-md border border-border bg-background/60 p-3",
                            p { class: "mb-2 text-xs uppercase tracking-[0.16em] text-muted-foreground", "RF samples" }
                            if details_rf.is_empty() {
                                p { class: "text-muted-foreground", "No noise / RSSI / SNR samples yet. Use Map → Probe noise." }
                            } else {
                                ul { class: "space-y-1 text-xs",
                                    for (noise, rssi, snr, at) in details_rf.iter() {
                                        li { class: "flex flex-wrap gap-x-3 gap-y-0.5 text-muted-foreground",
                                            span { "{format_time(Some(*at))}" }
                                            span { class: "tabular-nums text-foreground", "noise {noise} dBm" }
                                            span { class: "tabular-nums text-foreground", "rssi {rssi}" }
                                            span { class: "tabular-nums text-foreground", "snr {snr:.1}" }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "rounded-md border border-border bg-background/60 p-3",
                            p { class: "mb-2 text-xs uppercase tracking-[0.16em] text-muted-foreground", "Recent messages" }
                            if details_messages.is_empty() {
                                p { class: "text-muted-foreground", "No messages attributed to this pubkey yet." }
                            } else {
                                ul { class: "space-y-2",
                                    for msg in details_messages.iter() {
                                        li { class: "rounded border border-border/70 bg-card/40 px-2.5 py-2",
                                            div { class: "flex flex-wrap items-center gap-2 text-[0.7rem] text-muted-foreground",
                                                span { "{format_time(Some(msg.received_at))}" }
                                                span { class: "uppercase", "{msg.direction}" }
                                                if let Some(hops) = msg.hop_count {
                                                    span { "{hops} hop(s)" }
                                                }
                                                if let Some(snr) = msg.snr {
                                                    span { "snr {snr:.1}" }
                                                }
                                            }
                                            div { class: "mt-1 text-xs",
                                                MessageBody {
                                                    text: msg.text.clone(),
                                                    outgoing: msg.direction == "tx",
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        details { class: "rounded-md border border-border bg-background/60 p-3",
                            summary { class: "cursor-pointer text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                "Raw contact snapshot (JSON)"
                            }
                            pre { class: "mt-2 max-h-48 overflow-auto whitespace-pre-wrap break-all font-mono text-[0.7rem] text-muted-foreground",
                                "{details_raw_pretty}"
                            }
                        }
                        div { class: "flex justify-end",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| details_pubkey.set(None),
                                "Close"
                            }
                        }
                    }
                }
            }
            PageHeading { icon: IconName::Users, title: "Roster".to_string(),
                "Search heard nodes, send a direct message, or manually share a private channel secret."
            }
            Dialog {
                open: confirm_clear_new(),
                title: "Clear NEW badges".to_string(),
                on_close: move |_| confirm_clear_new.set(false),
                div { class: "flex flex-col gap-3",
                    p { class: "text-sm text-muted-foreground",
                        "Clear NEW on {unnoticed_count} node(s)? This only marks them as noticed — it does not remove anyone from the roster."
                    }
                    div { class: "flex justify-end gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| confirm_clear_new.set(false),
                            "Cancel"
                        }
                        Button {
                            onclick: move |_| {
                                match state().db.mark_all_nodes_noticed() {
                                    Ok(n) => {
                                        confirm_clear_new.set(false);
                                        note.set(if n == 0 {
                                            "No NEW badges to clear.".into()
                                        } else {
                                            format!("Cleared NEW on {n} node(s).")
                                        });
                                    }
                                    Err(err) => {
                                        confirm_clear_new.set(false);
                                        note.set(err.to_string());
                                    }
                                }
                            },
                            "Clear NEW"
                        }
                    }
                }
            }
            Dialog {
                open: confirm_prune_heard(),
                title: "Remove heard-only nodes".to_string(),
                on_close: move |_| confirm_prune_heard.set(false),
                div { class: "flex flex-col gap-3",
                    p { class: "text-sm text-muted-foreground",
                        "Remove {heard_only_count} heard-only node(s) from the roster? Enrolled, pending, denied, and HQ stay. Tracker, block, and claim rows for removed keys are cleared. Message history is kept (use Settings purge for that)."
                    }
                    div { class: "flex justify-end gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| confirm_prune_heard.set(false),
                            "Cancel"
                        }
                        Button {
                            kind: ButtonKind::Danger,
                            onclick: move |_| {
                                match state().db.prune_heard_nodes() {
                                    Ok(n) => {
                                        confirm_prune_heard.set(false);
                                        details_pubkey.set(None);
                                        note.set(if n == 0 {
                                            "No heard-only nodes to remove.".into()
                                        } else {
                                            format!("Removed {n} heard-only node(s).")
                                        });
                                    }
                                    Err(err) => {
                                        confirm_prune_heard.set(false);
                                        note.set(err.to_string());
                                    }
                                }
                            },
                            Icon { name: IconName::Trash, size: 14 }
                            "Remove heard-only"
                        }
                    }
                }
            }
            div { class: "flex flex-col gap-2 sm:flex-row sm:items-center",
                div { class: "min-w-0 flex-1",
                    TextInput {
                        value: query(),
                        placeholder: "Fuzzy search name, prefix, pubkey…".to_string(),
                        kind: "text".to_string(),
                        oninput: move |event: FormEvent| query.set(event.value()),
                    }
                }
                div { class: "flex flex-wrap items-center gap-2",
                    Button {
                        kind: ButtonKind::Ghost,
                        disabled: unnoticed_count <= 0,
                        onclick: move |_| confirm_clear_new.set(true),
                        "Clear NEW"
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        disabled: heard_only_count <= 0,
                        onclick: move |_| confirm_prune_heard.set(true),
                        "Remove heard-only"
                    }
                    p { class: "text-xs text-muted-foreground", "{ranked.len()} shown" }
                }
            }
            if !note().is_empty() {
                p { class: "text-xs text-amber-200", "{note}" }
            }
            div { class: "overflow-hidden rounded-lg border border-border",
                table { class: "w-full text-left text-sm",
                    thead { class: "bg-card text-xs uppercase tracking-wide text-muted-foreground",
                        tr {
                            th { class: "px-3 py-2 font-medium", "Name" }
                            th { class: "px-3 py-2 font-medium", "State" }
                            th { class: "px-3 py-2 font-medium", "Type" }
                            th { class: "px-3 py-2 font-medium", "Last heard" }
                            th { class: "px-3 py-2 font-medium", "GPS" }
                            th { class: "px-3 py-2 text-right font-medium", "Actions" }
                        }
                    }
                    tbody {
                        if ranked.is_empty() {
                            tr {
                                td { colspan: "6", class: "px-3 py-6 text-muted-foreground",
                                    span { class: "inline-flex items-center gap-2",
                                        Icon { name: IconName::Radio, size: 16 }
                                        if needle.trim().is_empty() {
                                            "No nodes yet. Connect the radio and wait for adverts."
                                        } else {
                                            "No roster matches for that search."
                                        }
                                    }
                                }
                            }
                        }
                        for (row_i, (_score, node, title)) in ranked.into_iter().enumerate() {
                            {
                                let is_claimed = claimed_keys.contains(&node.pubkey);
                                let can_claim = matches!(node.contact_type, 2 | 3 | 4)
                                    && node.enrollment != Enrollment::Hq
                                    && !is_claimed;
                                let row_class = if row_i % 2 == 1 {
                                    "row-zebra border-t border-border"
                                } else {
                                    "border-t border-border"
                                };
                                rsx! {
                            tr { class: "{row_class}",
                                td { class: "px-3 py-2",
                                    div { class: "flex flex-wrap items-center gap-2",
                                        span { class: "font-medium", "{title}" }
                                        if !node.noticed {
                                            span { class: "roster-new-pill", "NEW" }
                                        }
                                        if is_claimed {
                                            Badge { tone: "signal".to_string(), "Claimed" }
                                        }
                                    }
                                    div { class: "font-mono text-xs text-muted-foreground", "{node.prefix}" }
                                }
                                td { class: "px-3 py-2", Badge { tone: node.enrollment.as_str().to_string(), "{node.enrollment.label()}" } }
                                td { class: "px-3 py-2",
                                    ContactTypePill {
                                        enrollment: node.enrollment,
                                        contact_type: node.contact_type,
                                    }
                                }
                                td { class: "px-3 py-2", "{format_time(node.last_heard)}" }
                                td { class: "px-3 py-2",
                                    span {
                                        class: "inline-flex items-center gap-1.5 {heard_icon_class(node.last_heard)}",
                                        title: if heard_within_week(node.last_heard) {
                                            "Heard within the last week"
                                        } else {
                                            "Not heard in over a week"
                                        },
                                        Icon { name: IconName::Locate, size: 14 }
                                        if node.has_gps { "Fix" } else { "None" }
                                    }
                                }
                                td { class: "px-3 py-2",
                                    div { class: "roster-actions",
                                        button {
                                            r#type: "button",
                                            class: "roster-icon-btn roster-icon-btn-details",
                                            title: "Details",
                                            "aria-label": "Details",
                                            onclick: {
                                                let pubkey = node.pubkey.clone();
                                                move |_| {
                                                    let _ = state().db.mark_node_noticed(&pubkey);
                                                    details_pubkey.set(Some(pubkey.clone()));
                                                }
                                            },
                                            Icon { name: IconName::ChevronRight, size: 15 }
                                        }
                                        if can_claim {
                                            button {
                                                r#type: "button",
                                                class: "roster-icon-btn roster-icon-btn-claim",
                                                title: "Claim infrastructure…",
                                                "aria-label": "Claim infrastructure",
                                                onclick: {
                                                    let pubkey = node.pubkey.clone();
                                                    let label = title.clone();
                                                    move |_| {
                                                        claim_error.set(String::new());
                                                        claim_password.set(String::new());
                                                        claim_target.set(Some((pubkey.clone(), label.clone())));
                                                    }
                                                },
                                                Icon { name: IconName::Key, size: 15 }
                                            }
                                        }
                                        if node.enrollment != Enrollment::Hq {
                                            button {
                                                r#type: "button",
                                                class: "roster-icon-btn roster-icon-btn-message",
                                                title: "Direct message",
                                                "aria-label": "Direct message",
                                                onclick: {
                                                    let pubkey = node.pubkey.clone();
                                                    let title = title.clone();
                                                    move |_| {
                                                        dm_body.set(String::new());
                                                        note.set(String::new());
                                                        dm_target.set(Some((pubkey.clone(), title.clone())));
                                                    }
                                                },
                                                Icon { name: IconName::Messages, size: 15 }
                                            }
                                            button {
                                                r#type: "button",
                                                class: "roster-icon-btn roster-icon-btn-channel",
                                                title: "Share private channel…",
                                                "aria-label": "Share private channel",
                                                onclick: {
                                                    let pubkey = node.pubkey.clone();
                                                    let title = title.clone();
                                                    move |_| {
                                                        note.set(String::new());
                                                        invite_target.set(Some((pubkey.clone(), title.clone())));
                                                    }
                                                },
                                                Icon { name: IconName::Radio, size: 15 }
                                            }
                                        }
                                        if node.enrollment == Enrollment::Enrolled {
                                            button {
                                                r#type: "button",
                                                class: "roster-icon-btn roster-icon-btn-revoke",
                                                title: "Revoke enrollment",
                                                "aria-label": "Revoke enrollment",
                                                onclick: {
                                                    let pubkey = node.pubkey.clone();
                                                    let secrets = secrets.clone();
                                                    move |_| {
                                                        match state().db.revoke(&pubkey, &secrets) {
                                                            Ok(_) => note.set(
                                                                "Revoked and rotated private channel secrets."
                                                                    .into(),
                                                            ),
                                                            Err(err) => note.set(err.to_string()),
                                                        }
                                                    }
                                                },
                                                Icon { name: IconName::Ban, size: 15 }
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
                open: dm_target().is_some(),
                title: dm_target()
                    .as_ref()
                    .map(|(_, name)| format!("Direct message · {name}"))
                    .unwrap_or_else(|| "Direct message".into()),
                on_close: move |_| dm_target.set(None),
                div { class: "flex flex-col gap-3",
                    div { class: "flex items-stretch gap-2",
                        textarea {
                            class: "min-h-28 min-w-0 flex-1 rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground outline-none",
                            value: dm_body(),
                            placeholder: "Write a direct message…",
                            oninput: move |event: FormEvent| {
                                dm_body.set(clamp_mesh_text(event.value()));
                            },
                        }
                        div { class: "flex flex-col justify-end",
                            EmojiPickerButton {
                                align_end: true,
                                on_pick: move |emoji: String| {
                                    dm_body.set(append_emoji(&dm_body(), &emoji));
                                },
                            }
                        }
                    }
                    div { class: "flex items-center justify-between gap-2",
                        MeshTextCount { text: dm_body() }
                        div { class: "flex justify-end gap-2",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| dm_target.set(None),
                                "Cancel"
                            }
                            Button {
                                disabled: dm_body().trim().is_empty(),
                                onclick: move |_| {
                                    let Some((pubkey, _)) = dm_target() else { return; };
                                    let body = dm_body().trim().to_string();
                                    if body.is_empty() {
                                        return;
                                    }
                                    if body.len() > MESH_TEXT_MAX_BYTES {
                                        note.set(format!(
                                            "Mesh text is limited to {MESH_TEXT_MAX_BYTES} bytes."
                                        ));
                                        return;
                                    }
                                    match state().db.queue_text(&pubkey, &body, &body) {
                                        Ok(_) => {
                                            note.set("Direct message queued for the companion.".into());
                                            dm_target.set(None);
                                            dm_body.set(String::new());
                                        }
                                        Err(err) => note.set(err.to_string()),
                                    }
                                },
                                Icon { name: IconName::Messages, size: 14 }
                                "Send DM"
                            }
                        }
                    }
                }
            }
            Dialog {
                open: claim_target().is_some(),
                title: "Claim infrastructure".to_string(),
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
                            if let Some((_, label)) = claim_target() {
                                format!(
                                    "Enter the MeshCore admin password for {label}. Guest password is refused. Nothing is written until login succeeds."
                                )
                            } else {
                                "Enter the admin password.".to_string()
                            }
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
                            disabled: claim_busy()
                                || claim_password().trim().is_empty()
                                || claim_target().is_none(),
                            onclick: move |_| {
                                let Some((pubkey, label)) = claim_target() else {
                                    return;
                                };
                                let password = claim_password();
                                if password.trim().is_empty() {
                                    claim_error.set("Admin password required.".into());
                                    return;
                                }
                                if state().radio.snapshot().state != "connected" {
                                    claim_error.set(
                                        "Connect a primary companion before verifying.".into(),
                                    );
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
                                            match state().db.claim_infra_verified(
                                                &pubkey, None, &label, "",
                                            ) {
                                                Ok(()) => {
                                                    claim_target.set(None);
                                                    claim_password.set(String::new());
                                                    note.set(format!(
                                                        "Claimed and verified {label}."
                                                    ));
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
                            if claim_busy() { "Verifying…" } else { "Verify and claim" }
                        }
                    }
                }
            }
            Dialog {
                open: invite_target().is_some(),
                title: invite_target()
                    .as_ref()
                    .map(|(_, name)| format!("Add to private channel · {name}"))
                    .unwrap_or_else(|| "Add to private channel".into()),
                on_close: move |_| invite_target.set(None),
                div { class: "flex flex-col gap-3",
                    {
                        let info_name = state()
                            .db
                            .info_channel_name()
                            .unwrap_or_else(|_| crate::secrets::CHANNEL_NAME.into());
                        let net_name = state()
                            .db
                            .net_channel_name()
                            .unwrap_or_else(|_| crate::secrets::NET_CHANNEL_NAME.into());
                        rsx! {
                            p { class: "text-sm text-muted-foreground",
                                "Sends the channel secret by direct message, the same way signup approval does for "
                                code { class: "text-foreground", "{info_name}" }
                                " / "
                                code { class: "text-foreground", "{net_name}" }
                                "."
                            }
                        }
                    }
                    if private_channels.is_empty() {
                        p { class: "text-sm text-amber-200", "Create a private channel under Messages first." }
                    } else {
                        SelectInput {
                            id: "roster-invite-channel".to_string(),
                            value: invite_channel(),
                            onchange: move |event: FormEvent| invite_channel.set(event.value()),
                            for row in private_channels.iter().cloned() {
                                option {
                                    key: "{row.name}",
                                    value: "{row.name}",
                                    style: "background-color: #101418; color: #f4f4f5;",
                                    if row.role == "emergency" {
                                        "{row.name} (emergency)"
                                    } else {
                                        "{row.name}"
                                    }
                                }
                            }
                        }
                    }
                    div { class: "flex justify-end gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| invite_target.set(None),
                            "Cancel"
                        }
                        Button {
                            disabled: private_channels.is_empty() || invite_channel().trim().is_empty(),
                            onclick: move |_| {
                                let Some((pubkey, _)) = invite_target() else { return; };
                                let channel = invite_channel();
                                match state().db.invite_to_private_channel(
                                    &pubkey,
                                    &channel,
                                    &state().secrets,
                                ) {
                                    Ok(()) => {
                                        note.set(format!("Queued private-channel invite for {channel}."));
                                        invite_target.set(None);
                                    }
                                    Err(err) => note.set(err.to_string()),
                                }
                            },
                            Icon { name: IconName::Key, size: 14 }
                            "Send invite"
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn fuzzy_score(query: &str, fields: &[&str]) -> Option<i32> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let mut best = None;
    for field in fields {
        let hay = field.to_lowercase();
        if hay.is_empty() {
            continue;
        }
        if hay == query {
            best = Some(best.map_or(10_000, |score: i32| score.max(10_000)));
            continue;
        }
        if hay.starts_with(&query) {
            best = Some(best.map_or(8_000, |score: i32| score.max(8_000)));
            continue;
        }
        if hay.contains(&query) {
            let score = 5_000 - (hay.len() as i32 - query.len() as i32).clamp(0, 500);
            best = Some(best.map_or(score, |current| current.max(score)));
            continue;
        }
        if let Some(score) = fuzzy_subsequence_score(&query, &hay) {
            best = Some(best.map_or(score, |current| current.max(score)));
        }
    }
    best
}

fn fuzzy_subsequence_score(query: &str, haystack: &str) -> Option<i32> {
    let mut q_chars = query.chars();
    let Some(mut need) = q_chars.next() else {
        return Some(0);
    };
    let mut score = 0i32;
    let mut streak = 0i32;
    for ch in haystack.chars() {
        if ch == need {
            streak += 1;
            score += 12 + streak * 3;
            match q_chars.next() {
                Some(next) => need = next,
                None => return Some(score),
            }
        } else {
            streak = 0;
        }
    }
    None
}

fn contact_type_label(value: u8) -> &'static str {
    match value {
        1 => "Chat",
        2 => "Repeater",
        3 => "Room",
        4 => "Sensor",
        _ => "Node",
    }
}

fn roster_type_key(enrollment: Enrollment, contact_type: u8) -> &'static str {
    if enrollment == Enrollment::Hq {
        return "hq";
    }
    match contact_type {
        1 => "chat",
        2 => "repeater",
        3 => "room",
        4 => "sensor",
        _ => "node",
    }
}

fn roster_type_label(enrollment: Enrollment, contact_type: u8) -> &'static str {
    if enrollment == Enrollment::Hq {
        "HQ"
    } else {
        contact_type_label(contact_type)
    }
}

#[component]
fn ContactTypePill(enrollment: Enrollment, contact_type: u8) -> Element {
    let key = roster_type_key(enrollment, contact_type);
    let label = roster_type_label(enrollment, contact_type);
    rsx! {
        span { class: "roster-type-pill roster-type-pill-{key}", "{label}" }
    }
}

#[component]
fn InboxPage() -> Element {
    let mut refresh = use_signal(|| 0u64);
    let mut refresh_started = use_signal(|| false);
    if !refresh_started() {
        refresh_started.set(true);
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                refresh.set(refresh() + 1);
            }
        });
    }
    let _ = refresh();
    let pending = state().db.pending_nodes().unwrap_or_default();
    let pending_alerts = state()
        .db
        .list_pending_service_events()
        .unwrap_or_default();
    let secrets = state().secrets.clone();
    rsx! {
        div { class: "flex flex-col gap-3",
            {
                let info_name = state()
                    .db
                    .info_channel_name()
                    .unwrap_or_else(|_| crate::secrets::CHANNEL_NAME.into());
                let net_name = state()
                    .db
                    .net_channel_name()
                    .unwrap_or_else(|_| crate::secrets::NET_CHANNEL_NAME.into());
                rsx! {
                    PageHeading { icon: IconName::Inbox, title: "Inbox".to_string(),
                        "Operator queue for join requests and held community alerts. A node DMs #signup to ask to join; approval sends the {info_name} and {net_name} secrets. Weather and Amber wait here when auto-relay is off. Ordinary chats are under Messages → Direct."
                    }
                }
            }
            if pending.is_empty() {
                Card { title: "Waiting", icon: Some(IconName::Inbox),
                    p { class: "inline-flex items-center gap-2 text-sm text-muted-foreground",
                        Icon { name: IconName::Check, size: 16 }
                        "No pending join requests."
                    }
                }
            }
            for node in pending {
                Card { title: node_title(&node.name, &node.requested_name), icon: Some(IconName::Users),
                    p { class: "mb-3 font-mono text-xs text-muted-foreground", "{node.pubkey}" }
                    div { class: "flex gap-2",
                        Button {
                            onclick: {
                                let pubkey = node.pubkey.clone();
                                let secrets = secrets.clone();
                                move |_| {
                                    if let Err(err) = state().db.approve(&pubkey, &secrets) {
                                        eprintln!("approve: {err}");
                                    }
                                }
                            },
                            Icon { name: IconName::Check, size: 14 }
                            "Approve"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: {
                                let pubkey = node.pubkey.clone();
                                move |_| {
                                    let _ = state().db.deny(&pubkey);
                                }
                            },
                            Icon { name: IconName::Close, size: 14 }
                            "Deny"
                        }
                    }
                }
            }
            if !pending_alerts.is_empty() {
                PageHeading { icon: IconName::Inbox, title: "Pending alerts".to_string(),
                    "Relay sends the compact mesh line to the alerts channel. Dismiss drops it without transmitting."
                }
                for alert in pending_alerts {
                    Card {
                        title: format!("{} · {}", alert.kind.to_ascii_uppercase(), format_time(Some(alert.created_at))),
                        icon: Some(IconName::Radio),
                        p { class: "mb-3 text-sm whitespace-pre-wrap break-words", "{alert.body}" }
                        div { class: "flex gap-2",
                            Button {
                                onclick: {
                                    let id = alert.id.clone();
                                    let body = alert.body.clone();
                                    move |_| {
                                        if let Ok(Some(_)) = state().db.decide_service_event(&id, "sent") {
                                            let slot = crate::services::alerts_channel_slot(&state().db);
                                            let _ = state().radio.send_channel(slot, body.clone());
                                            let _ = state().db.audit("svc_nws_relay", "", &body);
                                            status_note("Alert queued to the alerts channel.");
                                        }
                                        refresh.set(refresh() + 1);
                                    }
                                },
                                Icon { name: IconName::Check, size: 14 }
                                "Relay"
                            }
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: {
                                    let id = alert.id.clone();
                                    move |_| {
                                        let _ = state().db.decide_service_event(&id, "dismissed");
                                        refresh.set(refresh() + 1);
                                    }
                                },
                                Icon { name: IconName::Close, size: 14 }
                                "Dismiss"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum MessagesRailConfirm {
    DeleteChannel(String),
    ClearDm { pubkey: String, name: String },
}

/// Picker dialogs opened from the compose + menu.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ComposePlusDialog {
    ShareChannel,
    ShareContact,
    ShareMapLocation,
    QuickDm,
}

/// Create / join channel slide-down steps.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ChannelDeskMode {
    Pick,
    Create,
    JoinPrivate,
    JoinPublic,
    JoinHash,
}

fn apply_compose_plus_action(
    action: ComposePlusAction,
    mut draft: Signal<String>,
    mut note: Signal<String>,
    mut plus_dialog: Signal<Option<ComposePlusDialog>>,
    mut plus_pick_query: Signal<String>,
    mut plus_pick_value: Signal<String>,
    mut plus_quick_body: Signal<String>,
    current_channel: Option<String>,
) {
    match action {
        ComposePlusAction::MyContact => match hq_contact_share_text() {
            Some(text) => {
                draft.set(insert_mesh_snippet(&draft(), &text));
                note.set(String::new());
            }
            None => note.set("Connect a companion so HQ contact info is available.".into()),
        },
        ComposePlusAction::MyPosition => match hq_position_share_text() {
            Some(text) => {
                draft.set(insert_mesh_snippet(&draft(), &text));
                note.set(String::new());
            }
            None => note.set(
                "No HQ position set. Set GPS under Settings → Device, or save a site pin."
                    .into(),
            ),
        },
        ComposePlusAction::ShareChannel => {
            plus_pick_query.set(String::new());
            plus_pick_value.set(current_channel.unwrap_or_default());
            plus_quick_body.set(String::new());
            plus_dialog.set(Some(ComposePlusDialog::ShareChannel));
        }
        ComposePlusAction::ShareContact => {
            plus_pick_query.set(String::new());
            plus_pick_value.set(String::new());
            plus_quick_body.set(String::new());
            plus_dialog.set(Some(ComposePlusDialog::ShareContact));
        }
        ComposePlusAction::ShareMapLocation => {
            plus_pick_query.set(String::new());
            plus_pick_value.set(String::new());
            plus_quick_body.set(String::new());
            plus_dialog.set(Some(ComposePlusDialog::ShareMapLocation));
        }
        ComposePlusAction::QuickDm => {
            plus_pick_query.set(String::new());
            plus_pick_value.set(String::new());
            plus_quick_body.set(String::new());
            plus_dialog.set(Some(ComposePlusDialog::QuickDm));
        }
    }
}

fn hq_contact_share_text() -> Option<String> {
    let radio = state().radio.snapshot();
    let key = radio.public_key.trim();
    if key.is_empty() {
        return None;
    }
    let name = state()
        .db
        .hq_advert_name()
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Metis HQ".into());
    Some(contact_add_uri(&name, key, 1))
}

fn hq_position_share_text() -> Option<String> {
    let radio = state().radio.snapshot();
    let label = state()
        .db
        .hq_advert_name()
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "HQ".into());
    if let (Some(lat), Some(lon)) = (radio.hq_lat, radio.hq_lon) {
        if let Some(text) = format_position_share(&label, lat, lon) {
            return Some(text);
        }
    }
    let lat = state()
        .db
        .setting("hq_site_lat")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<f64>().ok());
    let lon = state()
        .db
        .setting("hq_site_lon")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<f64>().ok());
    match (lat, lon) {
        (Some(lat), Some(lon)) => format_position_share(&format!("{label} site"), lat, lon),
        _ => None,
    }
}

#[component]
fn ComposePlusDialogs(
    dialog: Signal<Option<ComposePlusDialog>>,
    pick_query: Signal<String>,
    pick_value: Signal<String>,
    quick_body: Signal<String>,
    draft: Signal<String>,
    note: Signal<String>,
    mut refresh: Signal<u64>,
    mut selected_channel: Signal<Option<String>>,
    mut selected_dm: Signal<Option<String>>,
) -> Element {
    let mut dialog = dialog;
    let mut pick_query = pick_query;
    let mut pick_value = pick_value;
    let mut quick_body = quick_body;
    let mut draft = draft;
    let mut note = note;
    let open = dialog();
    let title = match open {
        Some(ComposePlusDialog::ShareChannel) => "Share channel",
        Some(ComposePlusDialog::ShareContact) => "Share contact",
        Some(ComposePlusDialog::ShareMapLocation) => "Share location from map",
        Some(ComposePlusDialog::QuickDm) => "Quick message (DM)",
        None => "",
    };
    let channels = state().db.list_channels().unwrap_or_default();
    let nodes = state().db.list_nodes().unwrap_or_default();
    let contact_choices: Vec<(String, String, u8)> = nodes
        .iter()
        .filter(|n| !n.pubkey.trim().is_empty())
        .filter(|n| n.enrollment != Enrollment::Hq)
        .map(|n| {
            let label = if n.name.trim().is_empty() {
                n.prefix.clone()
            } else {
                n.name.clone()
            };
            (n.pubkey.clone(), label, n.contact_type)
        })
        .collect();
    let map_locations: Vec<(String, String, f64, f64)> = {
        let mut rows = Vec::new();
        if let Some(text) = hq_position_share_text() {
            // Synthetic key for HQ current/site position.
            let _ = text;
            let radio = state().radio.snapshot();
            if let (Some(lat), Some(lon)) = (radio.hq_lat, radio.hq_lon) {
                if format_position_share("HQ", lat, lon).is_some() {
                    rows.push(("__hq__".into(), "HQ (companion)".into(), lat, lon));
                }
            } else if let (Some(lat), Some(lon)) = (
                state()
                    .db
                    .setting("hq_site_lat")
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse().ok()),
                state()
                    .db
                    .setting("hq_site_lon")
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse().ok()),
            ) {
                if format_position_share("HQ site", lat, lon).is_some() {
                    rows.push(("__hq_site__".into(), "HQ site pin".into(), lat, lon));
                }
            }
        }
        for n in nodes.iter() {
            let (Some(lat), Some(lon)) = (n.lat, n.lon) else {
                continue;
            };
            if format_position_share("x", lat, lon).is_none() {
                continue;
            }
            let label = if n.name.trim().is_empty() {
                n.prefix.clone()
            } else {
                n.name.clone()
            };
            rows.push((n.pubkey.clone(), label, lat, lon));
        }
        rows
    };
    rsx! {
        Dialog {
            open: open.is_some(),
            title: title.to_string(),
            on_close: move |_| {
                dialog.set(None);
                pick_query.set(String::new());
                pick_value.set(String::new());
                quick_body.set(String::new());
            },
            div { class: "flex flex-col gap-3",
                match open {
                    Some(ComposePlusDialog::ShareChannel) => rsx! {
                        p { class: "text-sm text-muted-foreground",
                            "Inserts the channel name into your message. Secrets are never shared in chat — use Roster → Invite for private-channel secrets."
                        }
                        SelectInput {
                            id: "compose-plus-channel".to_string(),
                            value: pick_value(),
                            onchange: move |event: FormEvent| pick_value.set(event.value()),
                            option { value: "", "Select a channel…" }
                            for ch in channels.iter() {
                                option { value: "{ch.name}", "{ch.name}" }
                            }
                        }
                        div { class: "flex justify-end gap-2",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| dialog.set(None),
                                "Cancel"
                            }
                            Button {
                                disabled: pick_value().trim().is_empty(),
                                onclick: move |_| {
                                    let name = pick_value();
                                    let text = format_channel_share(&name);
                                    if text.is_empty() {
                                        return;
                                    }
                                    draft.set(insert_mesh_snippet(&draft(), &text));
                                    dialog.set(None);
                                    note.set(String::new());
                                },
                                "Insert"
                            }
                        }
                    },
                    Some(ComposePlusDialog::ShareContact) => rsx! {
                        p { class: "text-sm text-muted-foreground",
                            "Inserts a meshcore:// contact link for the selected node."
                        }
                        TextInput {
                            value: pick_query(),
                            placeholder: "Search roster…",
                            oninput: move |event: FormEvent| pick_query.set(event.value()),
                        }
                        div { class: "dm-contact-results",
                            {
                                let needle = pick_query();
                                let mut ranked: Vec<(i32, String, String, u8)> = contact_choices
                                    .iter()
                                    .filter_map(|(pubkey, label, ty)| {
                                        let short = pubkey.chars().take(12).collect::<String>();
                                        let score = fuzzy_score(
                                            &needle,
                                            &[label.as_str(), pubkey.as_str(), short.as_str()],
                                        )?;
                                        Some((score, pubkey.clone(), label.clone(), *ty))
                                    })
                                    .collect();
                                ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.cmp(&b.2)));
                                ranked.truncate(40);
                                rsx! {
                                    if ranked.is_empty() {
                                        p { class: "px-3 py-2 text-sm text-muted-foreground", "No contacts match." }
                                    }
                                    for (_score, pubkey, label, ty) in ranked {
                                        {
                                            let active = pick_value() == pubkey;
                                            let key = pubkey.clone();
                                            rsx! {
                                                button {
                                                    r#type: "button",
                                                    class: if active {
                                                        "dm-contact-option dm-contact-option-active"
                                                    } else {
                                                        "dm-contact-option"
                                                    },
                                                    onclick: move |_| pick_value.set(key.clone()),
                                                    span { class: "font-medium", "{label}" }
                                                    span { class: "font-mono text-[0.7rem] text-muted-foreground",
                                                        "{pubkey.chars().take(12).collect::<String>()}… · type {ty}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "flex justify-end gap-2",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| dialog.set(None),
                                "Cancel"
                            }
                            Button {
                                disabled: pick_value().trim().is_empty(),
                                onclick: move |_| {
                                    let key = pick_value();
                                    let Some((_, label, ty)) =
                                        contact_choices.iter().find(|(p, _, _)| p == &key)
                                    else {
                                        return;
                                    };
                                    let uri = contact_add_uri(label, &key, *ty);
                                    draft.set(insert_mesh_snippet(&draft(), &uri));
                                    dialog.set(None);
                                    note.set(String::new());
                                },
                                "Insert"
                            }
                        }
                    },
                    Some(ComposePlusDialog::ShareMapLocation) => rsx! {
                        p { class: "text-sm text-muted-foreground",
                            "Pick a map pin with coordinates (self-reported GPS or HQ site). Ignores 0,0."
                        }
                        TextInput {
                            value: pick_query(),
                            placeholder: "Search pins…",
                            oninput: move |event: FormEvent| pick_query.set(event.value()),
                        }
                        div { class: "dm-contact-results",
                            {
                                let needle = pick_query();
                                let mut ranked: Vec<(i32, String, String, f64, f64)> = map_locations
                                    .iter()
                                    .filter_map(|(id, label, lat, lon)| {
                                        let score = fuzzy_score(
                                            &needle,
                                            &[label.as_str(), id.as_str()],
                                        )?;
                                        Some((score, id.clone(), label.clone(), *lat, *lon))
                                    })
                                    .collect();
                                ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.cmp(&b.2)));
                                ranked.truncate(40);
                                rsx! {
                                    if ranked.is_empty() {
                                        p { class: "px-3 py-2 text-sm text-muted-foreground",
                                            "No pins with GPS yet."
                                        }
                                    }
                                    for (_score, id, label, lat, lon) in ranked {
                                        {
                                            let active = pick_value() == id;
                                            let key = id.clone();
                                            rsx! {
                                                button {
                                                    r#type: "button",
                                                    class: if active {
                                                        "dm-contact-option dm-contact-option-active"
                                                    } else {
                                                        "dm-contact-option"
                                                    },
                                                    onclick: move |_| pick_value.set(key.clone()),
                                                    span { class: "font-medium", "{label}" }
                                                    span { class: "font-mono text-[0.7rem] text-muted-foreground",
                                                        "{lat:.5}, {lon:.5}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "flex justify-end gap-2",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| dialog.set(None),
                                "Cancel"
                            }
                            Button {
                                disabled: pick_value().trim().is_empty(),
                                onclick: move |_| {
                                    let key = pick_value();
                                    let Some((_, label, lat, lon)) =
                                        map_locations.iter().find(|(id, _, _, _)| id == &key)
                                    else {
                                        return;
                                    };
                                    if let Some(text) = format_position_share(label, *lat, *lon) {
                                        draft.set(insert_mesh_snippet(&draft(), &text));
                                        dialog.set(None);
                                        note.set(String::new());
                                    }
                                },
                                "Insert"
                            }
                        }
                    },
                    Some(ComposePlusDialog::QuickDm) => rsx! {
                        p { class: "text-sm text-muted-foreground",
                            "Queue a direct message without leaving this channel. Opens the DM thread after send."
                        }
                        TextInput {
                            value: pick_query(),
                            placeholder: "Search roster…",
                            oninput: move |event: FormEvent| pick_query.set(event.value()),
                        }
                        div { class: "dm-contact-results",
                            {
                                let needle = pick_query();
                                let mut ranked: Vec<(i32, String, String)> = contact_choices
                                    .iter()
                                    .filter_map(|(pubkey, label, _)| {
                                        let short = pubkey.chars().take(12).collect::<String>();
                                        let score = fuzzy_score(
                                            &needle,
                                            &[label.as_str(), pubkey.as_str(), short.as_str()],
                                        )?;
                                        Some((score, pubkey.clone(), label.clone()))
                                    })
                                    .collect();
                                ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.cmp(&b.2)));
                                ranked.truncate(40);
                                rsx! {
                                    if ranked.is_empty() {
                                        p { class: "px-3 py-2 text-sm text-muted-foreground", "No contacts match." }
                                    }
                                    for (_score, pubkey, label) in ranked {
                                        {
                                            let active = pick_value() == pubkey;
                                            let key = pubkey.clone();
                                            rsx! {
                                                button {
                                                    r#type: "button",
                                                    class: if active {
                                                        "dm-contact-option dm-contact-option-active"
                                                    } else {
                                                        "dm-contact-option"
                                                    },
                                                    onclick: move |_| pick_value.set(key.clone()),
                                                    span { class: "font-medium", "{label}" }
                                                    span { class: "font-mono text-[0.7rem] text-muted-foreground",
                                                        "{pubkey.chars().take(12).collect::<String>()}…"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        TextInput {
                            value: quick_body(),
                            placeholder: "Direct message…",
                            oninput: move |event: FormEvent| {
                                quick_body.set(clamp_mesh_text(event.value()));
                            },
                        }
                        MeshTextCount { text: quick_body() }
                        div { class: "flex justify-end gap-2",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| dialog.set(None),
                                "Cancel"
                            }
                            Button {
                                disabled: pick_value().trim().is_empty()
                                    || quick_body().trim().is_empty(),
                                onclick: move |_| {
                                    let peer = pick_value();
                                    let body = quick_body().trim().to_string();
                                    if peer.is_empty() || body.is_empty() {
                                        return;
                                    }
                                    match state().db.queue_text(&peer, &body, &body) {
                                        Ok(_) => {
                                            dialog.set(None);
                                            pick_query.set(String::new());
                                            pick_value.set(String::new());
                                            quick_body.set(String::new());
                                            selected_channel.set(None);
                                            selected_dm.set(Some(peer));
                                            refresh.set(refresh() + 1);
                                            status_note("Direct message queued for the companion.");
                                        }
                                        Err(err) => note.set(err.to_string()),
                                    }
                                },
                                Icon { name: IconName::Messages, size: 14 }
                                "Send DM"
                            }
                        }
                    },
                    None => rsx! {},
                }
            }
        }
    }
}

const MSG_WINDOW_INITIAL: i64 = 20;
const MSG_WINDOW_PAGE: i64 = 10;

#[derive(Clone, PartialEq, Eq)]
enum MsgWindowKey {
    /// Channel display name (not radio slot — slots remap on sync).
    Channel(String),
    Dm(String),
}

#[derive(Clone)]
struct MsgWindowState {
    key: MsgWindowKey,
    messages: Vec<crate::db::MessageRow>,
    has_older: bool,
}

fn unread_badge_class(unread_mentions: i64) -> &'static str {
    if unread_mentions > 0 {
        "channel-unread-badge channel-unread-badge-mention"
    } else {
        "channel-unread-badge"
    }
}

fn merge_newer_messages(
    window: &mut MsgWindowState,
    newest: Vec<crate::db::MessageRow>,
) -> bool {
    let last = window.messages.last().map(|m| m.id).unwrap_or(0);
    let mut changed = false;
    for message in newest {
        if message.id > last {
            window.messages.push(message);
            changed = true;
        }
    }
    changed
}

/// Confirm before dropping channel + DM traffic from a contact.
#[component]
fn BlockContactDialog(
    target: Signal<Option<(String, String)>>,
    on_blocked: EventHandler<()>,
) -> Element {
    let mut target = target;
    let open = target().is_some();
    let label = target()
        .as_ref()
        .map(|(_, name)| name.clone())
        .unwrap_or_default();
    let name_only = target()
        .as_ref()
        .is_some_and(|(key, _)| key.starts_with("name:"));
    rsx! {
        Dialog {
            open: open,
            title: "Block contact".to_string(),
            on_close: move |_| target.set(None),
            div { class: "flex flex-col gap-3",
                p { class: "text-sm text-muted-foreground",
                    if name_only {
                        "Block {label}? Future channel messages from this display name will be dropped. If HQ later hears their node, DMs from that pubkey are blocked too. Enrollment stays the same — unblock under Messages → Blocked."
                    } else {
                        "Block {label}? Future channel and direct messages from them will be dropped. Enrollment stays the same — you can unblock under Messages → Blocked."
                    }
                }
                div { class: "flex justify-end gap-2",
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| target.set(None),
                        "Cancel"
                    }
                    Button {
                        kind: ButtonKind::Danger,
                        onclick: move |_| {
                            let Some((key, name)) = target() else { return; };
                            let pubkey = if key.starts_with("name:") {
                                None
                            } else {
                                Some(key.as_str())
                            };
                            match state().db.block_sender(pubkey, &name) {
                                Ok(()) => {
                                    target.set(None);
                                    status_note(format!(
                                        "Blocked {name}. Future messages from them are dropped."
                                    ));
                                    on_blocked.call(());
                                }
                                Err(err) => {
                                    target.set(None);
                                    status_note(err.to_string());
                                }
                            }
                        },
                        Icon { name: IconName::Ban, size: 14 }
                        "Block"
                    }
                }
            }
        }
    }
}

/// Shown when channel Block has no display name to key off (rare malformed frame).
#[component]
fn BlockResolveFailDialog(target: Signal<Option<String>>) -> Element {
    let mut target = target;
    let open = target().is_some();
    rsx! {
        Dialog {
            open: open,
            title: "Can't block yet".to_string(),
            on_close: move |_| target.set(None),
            div { class: "flex flex-col gap-3",
                p { class: "text-sm text-muted-foreground",
                    "This channel message has no sender name to block. Open a DM with the node, or wait until a named channel frame arrives."
                }
                div { class: "flex justify-end",
                    Button {
                        onclick: move |_| target.set(None),
                        "OK"
                    }
                }
            }
        }
    }
}

fn messages_rail_width_px() -> f64 {
    state()
        .db
        .setting("messages_rail_width_px")
        .ok()
        .flatten()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(240.0)
        .clamp(176.0, 480.0)
}

fn persist_messages_rail_width(px: f64) {
    let px = px.round().clamp(176.0, 480.0);
    let _ = state()
        .db
        .set_setting("messages_rail_width_px", &format!("{px:.0}"));
}

fn set_messages_split_cursor(active: bool) {
    // WebKitGTK often ignores element CSS cursors. Hit the window API and force a
    // document-wide style (eval must be polled/awaited or the script never runs).
    #[cfg(feature = "desktop")]
    {
        use dioxus::desktop::tao::window::CursorIcon;
        let icon = if active {
            CursorIcon::ColResize
        } else {
            CursorIcon::Default
        };
        dioxus::desktop::window().set_cursor_icon(icon);
    }
    let script = if active {
        r#"(() => {
          document.documentElement.classList.add('hq-col-resize');
          document.body && (document.body.style.cursor = 'col-resize');
          let s = document.getElementById('hq-col-resize-style');
          if (!s) {
            s = document.createElement('style');
            s.id = 'hq-col-resize-style';
            document.head.appendChild(s);
          }
          s.textContent = 'html.hq-col-resize,html.hq-col-resize body,html.hq-col-resize *{cursor:col-resize!important;}';
        })()"#
    } else {
        r#"(() => {
          document.documentElement.classList.remove('hq-col-resize');
          if (document.body) document.body.style.cursor = '';
          const s = document.getElementById('hq-col-resize-style');
          if (s) s.textContent = '';
        })()"#
    };
    spawn(async move {
        let _ = document::eval(script).await;
    });
}

#[component]
fn MessagesThreadShell(
    channels: Vec<ChannelRow>,
    dm_threads: Vec<DmThreadRow>,
    selected_channel: Option<String>,
    selected_dm: Option<String>,
    linked: bool,
    rail_epoch: Signal<u64>,
    on_select_channel: EventHandler<String>,
    on_select_dm: EventHandler<String>,
    on_after_delete_channel: EventHandler<String>,
    on_after_clear_dm: EventHandler<String>,
    on_channel_appearance: EventHandler<String>,
    children: Element,
) -> Element {
    let mut confirm = use_signal(|| None::<MessagesRailConfirm>);
    let mut rail_width = use_signal(messages_rail_width_px);
    let mut split_drag = use_signal(|| None::<(f64, f64)>); // (start_x, start_width)
    let shell_style = format!("--messages-rail-width: {:.0}px;", rail_width());
    let dragging = split_drag().is_some();
    let shell_class = if dragging {
        "messages-shell hq-page-fill messages-shell-resizing"
    } else {
        "messages-shell hq-page-fill"
    };
    rsx! {
        div {
            class: "{shell_class}",
            style: "{shell_style}",
            onpointermove: move |event: PointerEvent| {
                let Some((start_x, start_w)) = split_drag() else {
                    return;
                };
                let pt = event.data().client_coordinates();
                let next = (start_w + (pt.x - start_x)).clamp(176.0, 480.0);
                rail_width.set(next);
            },
            onpointerup: move |_| {
                if split_drag().is_some() {
                    persist_messages_rail_width(rail_width());
                    split_drag.set(None);
                    set_messages_split_cursor(false);
                }
            },
            onpointercancel: move |_| {
                if split_drag().is_some() {
                    persist_messages_rail_width(rail_width());
                    split_drag.set(None);
                    set_messages_split_cursor(false);
                }
            },
            messages_rail::MessagesRail {
                channels: channels,
                dm_threads: dm_threads,
                selected_channel: selected_channel.clone(),
                selected_dm: selected_dm.clone(),
                on_select_channel: move |name| on_select_channel.call(name),
                on_select_dm: move |pubkey| on_select_dm.call(pubkey),
                on_delete_channel: move |name| {
                    confirm.set(Some(MessagesRailConfirm::DeleteChannel(name)));
                },
                on_clear_dm: move |(pubkey, name)| {
                    confirm.set(Some(MessagesRailConfirm::ClearDm { pubkey, name }));
                },
                on_channel_appearance: move |name| on_channel_appearance.call(name),
                rail_epoch,
            }
            div {
                class: "messages-split-handle",
                style: "cursor: col-resize;",
                title: "Drag to resize",
                onmousemove: move |_| {
                    if split_drag().is_none() {
                        set_messages_split_cursor(true);
                    }
                },
                role: "separator",
                "aria-orientation": "vertical",
                "aria-label": "Resize conversations list",
                onmouseenter: move |_| {
                    if split_drag().is_none() {
                        set_messages_split_cursor(true);
                    }
                },
                onmouseleave: move |_| {
                    if split_drag().is_none() {
                        set_messages_split_cursor(false);
                    }
                },
                onpointerenter: move |_| {
                    if split_drag().is_none() {
                        set_messages_split_cursor(true);
                    }
                },
                onpointerleave: move |_| {
                    if split_drag().is_none() {
                        set_messages_split_cursor(false);
                    }
                },
                onpointerdown: move |event: PointerEvent| {
                    event.prevent_default();
                    event.stop_propagation();
                    let pt = event.data().client_coordinates();
                    split_drag.set(Some((pt.x, rail_width())));
                    set_messages_split_cursor(true);
                },
                // Visible grip so the hit target is obvious (not only a CSS ::after).
                span {
                    class: "messages-split-handle-grip",
                    "aria-hidden": "true",
                }
            }
            div { class: "messages-convo",
                {children}
            }
            // Always mounted so the first drag frame is not lost waiting for a re-render.
            div {
                class: if dragging {
                    "messages-split-drag-layer"
                } else {
                    "messages-split-drag-layer messages-split-drag-layer-idle"
                },
                onpointermove: move |event: PointerEvent| {
                    let Some((start_x, start_w)) = split_drag() else {
                        return;
                    };
                    let pt = event.data().client_coordinates();
                    let next = (start_w + (pt.x - start_x)).clamp(176.0, 480.0);
                    rail_width.set(next);
                },
                onpointerup: move |_| {
                    if split_drag().is_some() {
                        persist_messages_rail_width(rail_width());
                        split_drag.set(None);
                        set_messages_split_cursor(false);
                    }
                },
                onpointercancel: move |_| {
                    if split_drag().is_some() {
                        persist_messages_rail_width(rail_width());
                        split_drag.set(None);
                        set_messages_split_cursor(false);
                    }
                },
            }
            Dialog {
                open: confirm().is_some(),
                title: match confirm() {
                    Some(MessagesRailConfirm::DeleteChannel(_)) => "Delete channel".into(),
                    Some(MessagesRailConfirm::ClearDm { .. }) => "Clear conversation".into(),
                    None => String::new(),
                },
                on_close: move |_| confirm.set(None),
                div { class: "flex flex-col gap-3",
                    p { class: "text-sm text-muted-foreground",
                        match confirm() {
                            Some(MessagesRailConfirm::DeleteChannel(name)) => format!(
                                "Remove {name} from HQ? System channels cannot be deleted. The companion slot will be cleared when connected."
                            ),
                            Some(MessagesRailConfirm::ClearDm { name, .. }) => format!(
                                "Delete all direct messages with {name}? The node stays on the roster."
                            ),
                            None => String::new(),
                        }
                    }
                    div { class: "flex justify-end gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| confirm.set(None),
                            "Cancel"
                        }
                        Button {
                            kind: ButtonKind::Danger,
                            onclick: move |_| {
                                match confirm() {
                                    Some(MessagesRailConfirm::DeleteChannel(name)) => {
                                        match state().db.soft_delete_channel(&name) {
                                            Ok(slot) => {
                                                if linked {
                                                    status_note(format!(
                                                        "Deleted {name}. Clearing companion slot {slot}…"
                                                    ));
                                                    let reply = state().radio.clear_channel(slot as u8);
                                                    let label = name.clone();
                                                    spawn(async move {
                                                        match reply.await {
                                                            Ok(Ok(())) => status_note(format!(
                                                                "Deleted {label} and cleared companion slot."
                                                            )),
                                                            Ok(Err(err)) => status_note(format!(
                                                                "Deleted {label} locally, but companion clear failed: {err}"
                                                            )),
                                                            Err(_) => status_note(format!(
                                                                "Deleted {label} locally; companion clear did not finish."
                                                            )),
                                                        }
                                                    });
                                                } else {
                                                    status_note(format!(
                                                        "Deleted {name}. Connect the companion to clear it from the radio too."
                                                    ));
                                                }
                                                confirm.set(None);
                                                on_after_delete_channel.call(name);
                                            }
                                            Err(err) => {
                                                confirm.set(None);
                                                status_note(err.to_string());
                                            }
                                        }
                                    }
                                    Some(MessagesRailConfirm::ClearDm { pubkey, name }) => {
                                        match state().db.clear_dm_thread(&pubkey) {
                                            Ok(_) => {
                                                confirm.set(None);
                                                status_note(format!("Cleared conversation with {name}."));
                                                on_after_clear_dm.call(pubkey);
                                            }
                                            Err(err) => {
                                                confirm.set(None);
                                                status_note(err.to_string());
                                            }
                                        }
                                    }
                                    None => {}
                                }
                            },
                            Icon { name: IconName::Trash, size: 14 }
                            match confirm() {
                                Some(MessagesRailConfirm::ClearDm { .. }) => "Clear",
                                _ => "Delete",
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn MessagesPage() -> Element {
    // Select by name, not slot: radio sync remaps slots and used to make a
    // click on Public open METIS-INFO when both rows shared slot 1.
    // Parent remounts this page (key = messages_root_epoch) when the nav
    // Messages item is clicked, which returns to the root list cleanly.
    let mut messages_tab = use_signal(|| "channels".to_string());
    let mut selected_channel = use_signal(|| None::<String>);
    let mut selected_dm = use_signal(|| None::<String>);
    let mut draft = use_signal(String::new);
    let mut channel_desk_open = use_signal(|| false);
    let mut keep_desk_open = use_signal(|| false);
    let mut channel_desk_mode = use_signal(|| ChannelDeskMode::Pick);
    let mut create_name = use_signal(String::new);
    let mut create_visibility = use_signal(|| "private".to_string());
    let mut create_advanced = use_signal(|| false);
    let mut create_region = use_signal(String::new);
    let mut join_private_name = use_signal(String::new);
    let mut join_private_secret = use_signal(String::new);
    let mut join_hash_name = use_signal(String::new);
    let mut note = use_signal(String::new);
    let sending = use_signal(|| false);
    let mut reply_target = use_signal(|| None::<MessageReplyTarget>);
    let mut refresh = use_signal(|| 0u64);
    let mut refresh_started = use_signal(|| false);
    let mut rail_epoch = use_signal(|| 0u64);
    let mut last_send_seq = use_signal(|| 0u64);
    let mut read_session = use_signal(|| None::<ChannelReadSession>);
    let mut new_dm_open = use_signal(|| false);
    let mut new_dm_peer = use_signal(String::new);
    let mut new_dm_query = use_signal(String::new);
    let mut new_dm_body = use_signal(String::new);
    let mut block_confirm = use_signal(|| None::<(String, String)>);
    let mut block_resolve_fail = use_signal(|| None::<String>);
    let plus_dialog = use_signal(|| None::<ComposePlusDialog>);
    let plus_pick_query = use_signal(String::new);
    let plus_pick_value = use_signal(String::new);
    let plus_quick_body = use_signal(String::new);
    let mut appearance_edit = use_signal(|| None::<AppearanceEditTarget>);
    let mut msg_window = use_signal(|| None::<MsgWindowState>);
    let mut loading_older = use_signal(|| false);
    let mut scroll_preserve_height = use_signal(|| None::<f64>);
    let mut message_keep = use_signal(|| {
        state()
            .db
            .message_keep()
            .unwrap_or_else(|_| "forever".into())
    });
    let mut keep_applied_at = use_signal(|| 0i64);
    if !refresh_started() {
        refresh_started.set(true);
        spawn(async move {
            loop {
                // Keep this gentle: full channel paints are expensive in the WebView.
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                refresh.set(refresh() + 1);
            }
        });
    }
    let _ = refresh();
    // Enforce keep window at most once a minute while Channels is open.
    let now = crate::db::unix_now();
    if now.saturating_sub(keep_applied_at()) >= 60 {
        keep_applied_at.set(now);
        let _ = state().db.apply_message_keep();
    }
    let radio = state().radio.snapshot();
    // Channel sends are fire-and-forget; surface the radio task result here.
    if radio.send_seq != last_send_seq() {
        last_send_seq.set(radio.send_seq);
        if radio.send_note.is_empty() {
            note.set(String::new());
        } else {
            note.set(radio.send_note.clone());
        }
    }
    let channels = state().db.list_channels().unwrap_or_default();
    let dm_threads = state().db.list_dm_threads().unwrap_or_default();
    let channel_unread_total: i64 = channels.iter().map(|c| c.unread_count).sum();
    let dm_unread_total: i64 = dm_threads.iter().map(|t| t.unread_count).sum();
    let channel_mention_total: i64 = channels.iter().map(|c| c.unread_mentions).sum();
    let dm_mention_total: i64 = dm_threads.iter().map(|t| t.unread_mentions).sum();
    // Bootstrap keeps state as "connecting" until the session loop is Ready, so
    // compose stays off during the first contact dump. Do not also require
    // sync_phase Ready here — a background contact sync (or a stuck Contacts
    // phase after Refresh) would disable Send while the companion is live.
    let linked = radio.state == "connected";
    let syncing = radio.sync_phase.is_active();

    // Restore scroll after prepending older messages.
    if let Some(prev_height) = scroll_preserve_height() {
        scroll_preserve_height.set(None);
        spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            let _ = document::eval(&format!(
                r#"(() => {{
                  const s = document.getElementById('channel-scroller')
                    || document.getElementById('dm-scroller');
                  if (!s) return;
                  s.scrollTop = Math.max(0, s.scrollHeight - {prev_height});
                }})()"#
            ));
        });
    }

    if let Some(dm_pubkey) = selected_dm() {
        let thread = dm_threads
            .iter()
            .find(|row| row.pubkey == dm_pubkey)
            .cloned();
        let node = state().db.get_node(&dm_pubkey).ok().flatten();
        let title = node
            .as_ref()
            .map(|n| node_title(&n.name, &n.requested_name))
            .or_else(|| thread.as_ref().map(|t| t.name.clone()))
            .unwrap_or_else(|| {
                dm_pubkey.chars().take(12).collect::<String>()
            });
        let window_key = MsgWindowKey::Dm(dm_pubkey.clone());
        // Paint from locals — reading msg_window() right after a set during
        // render can still be empty when switching threads in-place.
        let (messages, has_older) = match msg_window() {
            Some(mut window) if window.key == window_key => {
                if let Ok((newest, _)) =
                    state().db.list_dm_messages(&dm_pubkey, MSG_WINDOW_INITIAL)
                {
                    if merge_newer_messages(&mut window, newest) {
                        msg_window.set(Some(window.clone()));
                    }
                }
                (window.messages, window.has_older)
            }
            _ => {
                let (messages, has_older) = state()
                    .db
                    .list_dm_messages(&dm_pubkey, MSG_WINDOW_INITIAL)
                    .unwrap_or_else(|_| (Vec::new(), false));
                msg_window.set(Some(MsgWindowState {
                    key: window_key.clone(),
                    messages: messages.clone(),
                    has_older,
                }));
                (messages, has_older)
            }
        };
        let latest_id = messages.last().map(|m| m.id);
        let anchor = state()
            .db
            .dm_last_read(&dm_pubkey)
            .ok()
            .flatten()
            .filter(|id| messages.iter().any(|m| m.id == *id));
        let new_count = match anchor {
            Some(id) => messages.iter().filter(|m| m.id > id).count(),
            None => 0,
        };
        let show_banner = anchor.is_some() && new_count > 0;
        if !show_banner {
            if let Some(id) = latest_id {
                let _ = state().db.set_dm_last_read(&dm_pubkey, id);
            }
        }
        let dm_key = dm_pubkey.clone();
        let dm_appearance = state()
            .db
            .messages_appearance()
            .unwrap_or_default();
        let dm_pane_style = dm_appearance.pane_style();
        let dm_pane_class = if dm_appearance.bg_opacity > 0 {
            "min-h-0 flex-1 space-y-3 overflow-auto p-4 messages-pane-themed message-pane-boost-bubbles"
        } else if dm_appearance.themed() {
            "min-h-0 flex-1 space-y-3 overflow-auto p-4 messages-pane-themed"
        } else {
            "min-h-0 flex-1 space-y-3 overflow-auto p-4"
        };
        return rsx! {
            MessagesThreadShell {
                channels: channels.clone(),
                dm_threads: dm_threads.clone(),
                selected_channel: None,
                selected_dm: Some(dm_pubkey.clone()),
                linked,
                rail_epoch,
                on_select_channel: move |name: String| {
                    selected_dm.set(None);
                    selected_channel.set(Some(name));
                    draft.set(String::new());
                    reply_target.set(None);
                    read_session.set(None);
                    msg_window.set(None);
                    note.set(String::new());
                },
                on_select_dm: move |pubkey: String| {
                    selected_channel.set(None);
                    selected_dm.set(Some(pubkey));
                    draft.set(String::new());
                    reply_target.set(None);
                    msg_window.set(None);
                    note.set(String::new());
                },
                on_after_delete_channel: move |name: String| {
                    if selected_channel() == Some(name) {
                        selected_channel.set(None);
                        reply_target.set(None);
                        read_session.set(None);
                        msg_window.set(None);
                        draft.set(String::new());
                    }
                    refresh.set(refresh() + 1);
                },
                on_after_clear_dm: move |pubkey: String| {
                    if selected_dm() == Some(pubkey) {
                        selected_dm.set(None);
                        msg_window.set(None);
                        draft.set(String::new());
                    }
                    refresh.set(refresh() + 1);
                },
                on_channel_appearance: move |name: String| {
                    appearance_edit.set(Some(AppearanceEditTarget::Channel(name)));
                },
                AppearanceEditor {
                    target: appearance_edit,
                    on_saved: move |_| refresh.set(refresh() + 1),
                }
                div { class: "flex flex-wrap items-center justify-between gap-3",
                    div { class: "flex min-w-0 items-center gap-3",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| {
                                let _ = state().db.mark_dm_caught_up(&dm_key);
                                selected_dm.set(None);
                                msg_window.set(None);
                                draft.set(String::new());
                                note.set(String::new());
                            },
                            Icon { name: IconName::ChevronRight, size: 14 }
                            "Direct"
                        }
                        div { class: "min-w-0",
                            h3 { class: "truncate text-lg font-semibold", "{title}" }
                            p { class: "font-mono text-xs text-muted-foreground",
                                "{dm_pubkey.chars().take(12).collect::<String>()}"
                            }
                        }
                    }
                    {
                        let peer = dm_pubkey.clone();
                        let label = title.clone();
                        let already = state().db.is_blocked(&peer).unwrap_or(false);
                        rsx! {
                            Button {
                                kind: ButtonKind::Ghost,
                                disabled: already,
                                onclick: move |_| {
                                    block_confirm.set(Some((peer.clone(), label.clone())));
                                },
                                Icon { name: IconName::Ban, size: 14 }
                                if already { "Blocked" } else { "Block" }
                            }
                        }
                    }
                }
                BlockContactDialog {
                    target: block_confirm,
                    on_blocked: move |_| refresh.set(refresh() + 1),
                }
                BlockResolveFailDialog { target: block_resolve_fail }
                ComposePlusDialogs {
                    dialog: plus_dialog,
                    pick_query: plus_pick_query,
                    pick_value: plus_pick_value,
                    quick_body: plus_quick_body,
                    draft,
                    note,
                    refresh,
                    selected_channel,
                    selected_dm,
                }
                div { class: "flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border border-border bg-card",
                    div {
                        id: "dm-scroller",
                        key: "dm-scroller-{dm_pubkey}",
                        class: "{dm_pane_class}",
                        style: "{dm_pane_style}",
                        onscroll: {
                            let peer = dm_pubkey.clone();
                            move |_| {
                                if !has_older || loading_older() {
                                    return;
                                }
                                let peer = peer.clone();
                                spawn(async move {
                                    let near_top = document::eval(
                                        r#"return (() => {
                                          const s = document.getElementById('dm-scroller');
                                          return !!(s && s.scrollTop < 80);
                                        })();"#,
                                    )
                                    .await
                                    .ok()
                                    .and_then(|v| v.as_bool())
                                        == Some(true);
                                    if !near_top || loading_older() {
                                        return;
                                    }
                                    let Some(window) = msg_window() else { return };
                                    let Some(before_id) = window.messages.first().map(|m| m.id) else {
                                        return;
                                    };
                                    let prev_height = document::eval(
                                        r#"return document.getElementById('dm-scroller')?.scrollHeight ?? 0;"#,
                                    )
                                    .await
                                    .ok()
                                    .and_then(|v| v.as_f64())
                                    .unwrap_or(0.0);
                                    loading_older.set(true);
                                    match state().db.list_dm_messages_before(
                                        &peer,
                                        before_id,
                                        MSG_WINDOW_PAGE,
                                    ) {
                                        Ok((older, still_older)) if !older.is_empty() => {
                                            let mut next = window;
                                            let mut combined = older;
                                            combined.append(&mut next.messages);
                                            next.messages = combined;
                                            next.has_older = still_older;
                                            scroll_preserve_height.set(Some(prev_height));
                                            msg_window.set(Some(next));
                                        }
                                        Ok(_) => {
                                            if let Some(mut next) = msg_window() {
                                                next.has_older = false;
                                                msg_window.set(Some(next));
                                            }
                                        }
                                        Err(_) => {}
                                    }
                                    loading_older.set(false);
                                });
                            }
                        },
                        if has_older {
                            p { class: "text-center text-[0.7rem] text-muted-foreground",
                                if loading_older() { "Loading earlier messages…" } else { "Scroll up for earlier messages" }
                            }
                        }
                        if messages.is_empty() {
                            p { class: "text-sm text-muted-foreground", "No direct messages yet." }
                        }
                        for message in messages {
                            {
                                let outgoing = message.direction == "tx";
                                let show_new = show_banner && anchor == Some(message.id);
                                let peer_seed = dm_pubkey.clone();
                                let peer_label = title.clone();
                                let peer_tone = conversation_avatar_tone(&peer_seed);
                                rsx! {
                                    div {
                                        key: "{message.id}",
                                        class: "flex w-full flex-col gap-1",
                                        div {
                                            class: if outgoing {
                                                "ml-auto flex max-w-[85%] flex-col items-end gap-1"
                                            } else {
                                                "mr-auto flex max-w-[85%] flex-col items-start gap-1"
                                            },
                                            p { class: "message-meta-time",
                                                "{format_time(Some(message.received_at))}"
                                            }
                                            div {
                                                class: if outgoing {
                                                    "message-bubble min-w-[12rem] rounded-2xl rounded-br-md border border-sky-500/30 bg-[#16324f] text-sm text-sky-50"
                                                } else {
                                                    "message-bubble min-w-[12rem] rounded-2xl rounded-bl-md message-bubble-tone-{peer_tone} text-sm"
                                                },
                                                div { class: "message-meta-head",
                                                    if !outgoing {
                                                        ConversationAvatar {
                                                            label: peer_label.clone(),
                                                            seed: peer_seed.clone(),
                                                            large: true,
                                                        }
                                                    }
                                                    span { class: "message-meta-name", title: "{peer_label}",
                                                        if outgoing { "You" } else { "{peer_label}" }
                                                    }
                                                }
                                                MessageBody {
                                                    text: message.text.clone(),
                                                    outgoing,
                                                }
                                                if !outgoing {
                                                    div { class: "message-actions",
                                                        button {
                                                            class: "message-action-btn",
                                                            title: "Favorite this contact",
                                                            onclick: {
                                                                let pk = dm_pubkey.clone();
                                                                let nm = title.clone();
                                                                move |_| {
                                                                    match messages_rail::toggle_contact_favorite(
                                                                        &pk, &nm,
                                                                    ) {
                                                                        Ok((on, name_only)) => {
                                                                            let msg = if on && name_only {
                                                                                "Saved to Contacts → Pinned (no public key yet — upgrades when they advert)."
                                                                            } else if on {
                                                                                "Contact added to Favorites."
                                                                            } else {
                                                                                "Contact removed from Favorites."
                                                                            };
                                                                            status_note(msg);
                                                                            rail_epoch.with_mut(|n| {
                                                                                *n = n.wrapping_add(1)
                                                                            });
                                                                            refresh.set(refresh() + 1);
                                                                        }
                                                                        Err(err) => status_note(err),
                                                                    }
                                                                }
                                                            },
                                                            Icon { name: IconName::Star, size: 12 }
                                                            "Favorite"
                                                        }
                                                    }
                                                }
                                            }
                                            div { class: "message-meta-foot",
                                                if outgoing {
                                                    {
                                                        let (label, icon, pill) = match message.delivery.as_str() {
                                                            "queued" => ("Queued", IconName::Clock, "message-meta-pill message-meta-pill-ok"),
                                                            "sending" => ("Sending…", IconName::Clock, "message-meta-pill message-meta-pill-ok"),
                                                            "failed" => ("Not sent", IconName::Ban, "message-meta-pill message-meta-pill-bad"),
                                                            _ => ("Sent", IconName::Check, "message-meta-pill message-meta-pill-ok"),
                                                        };
                                                        rsx! {
                                                            span { class: "{pill}",
                                                                Icon { name: icon, size: 11 }
                                                                "{label}"
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    span { class: "message-meta-pill",
                                                        Icon { name: IconName::Signal, size: 11 }
                                                        if let Some(hops) = message.hop_count {
                                                            "{hops} hops"
                                                        } else {
                                                            "hops —"
                                                        }
                                                    }
                                                    span { class: "message-meta-pill", "Received" }
                                                }
                                            }
                                        }
                                        if show_new {
                                            span { class: "text-[0.7rem] font-medium text-signal",
                                                if new_count == 1 { "New messages · 1" } else { "New messages · {new_count}" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "border-t border-border p-3",
                        div { class: "flex gap-2",
                            ComposePlusButton {
                                disabled: !linked,
                                show_quick_dm: false,
                                on_action: move |action| {
                                    apply_compose_plus_action(
                                        action,
                                        draft,
                                        note,
                                        plus_dialog,
                                        plus_pick_query,
                                        plus_pick_value,
                                        plus_quick_body,
                                        None,
                                    );
                                },
                            }
                            input {
                                class: "h-10 min-w-0 flex-1 rounded-md border border-border bg-background px-3 text-sm text-foreground outline-none",
                                value: draft(),
                                placeholder: "Direct message…",
                                oninput: move |event: FormEvent| {
                                    draft.set(clamp_mesh_text(event.value()));
                                },
                                onkeydown: {
                                    let peer = dm_pubkey.clone();
                                    move |event: KeyboardEvent| {
                                        if event.key() == Key::Enter {
                                            event.prevent_default();
                                            let body = draft().trim().to_string();
                                            if body.is_empty() || !linked {
                                                return;
                                            }
                                            match state().db.queue_text(&peer, &body, &body) {
                                                Ok(_) => {
                                                    draft.set(String::new());
                                                    note.set(String::new());
                                                    refresh.set(refresh() + 1);
                                                }
                                                Err(err) => note.set(err.to_string()),
                                            }
                                        }
                                    }
                                },
                            }
                            Button {
                                disabled: !linked || draft().trim().is_empty(),
                                onclick: {
                                    let peer = dm_pubkey.clone();
                                    move |_| {
                                        let body = draft().trim().to_string();
                                        if body.is_empty() {
                                            return;
                                        }
                                        match state().db.queue_text(&peer, &body, &body) {
                                            Ok(_) => {
                                                draft.set(String::new());
                                                note.set(String::new());
                                                refresh.set(refresh() + 1);
                                            }
                                            Err(err) => note.set(err.to_string()),
                                        }
                                    }
                                },
                                Icon { name: IconName::Messages, size: 14 }
                                "Send"
                            }
                        }
                        div { class: "mt-2 flex flex-wrap items-center justify-between gap-2",
                            MeshTextCount { text: draft() }
                            if !linked {
                                p { class: "text-xs text-muted-foreground",
                                    if syncing {
                                        "Companion is still syncing — compose unlocks when Connected."
                                    } else {
                                        "Connect a companion to send direct messages."
                                    }
                                }
                            }
                        }
                        if !note().is_empty() {
                            p { class: "mt-2 text-xs text-amber-200", "{note}" }
                        }
                    }
                }
            }
        };
    }

    if let Some(selected_name) = selected_channel() {
        let channel = channels
            .iter()
            .find(|row| row.name == selected_name)
            .cloned();
        let Some(channel) = channel else {
            selected_channel.set(None);
            return rsx! {
                div { class: "text-sm text-muted-foreground", "Channel not found." }
            };
        };
        let slot = channel.slot;
        let window_key = MsgWindowKey::Channel(selected_name.clone());
        // Paint from locals when the cached window is for another channel.
        // Setting msg_window during render and then reading it for this paint
        // used to leave the thread blank until leave/re-enter remounted.
        let (messages, has_older) = match msg_window() {
            Some(mut window) if window.key == window_key => {
                if let Ok((newest, _)) =
                    state().db.list_channel_messages(slot, MSG_WINDOW_INITIAL)
                {
                    if merge_newer_messages(&mut window, newest) {
                        msg_window.set(Some(window.clone()));
                    }
                }
                (window.messages, window.has_older)
            }
            _ => {
                let (messages, has_older) = state()
                    .db
                    .list_channel_messages(slot, MSG_WINDOW_INITIAL)
                    .unwrap_or_else(|_| (Vec::new(), false));
                msg_window.set(Some(MsgWindowState {
                    key: window_key.clone(),
                    messages: messages.clone(),
                    has_older,
                }));
                (messages, has_older)
            }
        };
        let emergency = channel.role == "emergency";
        let _latest_id = messages.last().map(|message| message.id);
        let needs_session = read_session()
            .as_ref()
            .map(|session| session.slot != slot || session.name != selected_name)
            .unwrap_or(true);
        if needs_session {
            // Keep the raw last-read id even when it is older than the loaded
            // window (initial load is only MSG_WINDOW_INITIAL rows). Dropping it
            // left show_banner=true with no place to draw the divider.
            let stored_read = state().db.channel_last_read(slot).ok().flatten();
            // Match nav badge: RX + bot/service TX (origin=bot). Operator Compose TX does not.
            let new_count = channel.unread_count.max(0) as usize;
            let show_banner = new_count > 0;
            // Do not advance last-read just because the thread opened with no
            // unread — live traffic while open must still raise the divider + badge.
            read_session.set(Some(ChannelReadSession {
                name: selected_name.clone(),
                slot,
                anchor_id: stored_read,
                show_banner,
                new_count,
                scrolled: false,
            }));
            let scroll_anchor = stored_read;
            let scroll_to_bottom = !show_banner;
            let anchor_in_window = scroll_anchor
                .filter(|id| *id != 0)
                .is_some_and(|id| messages.iter().any(|m| m.id == id));
            spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                if scroll_to_bottom {
                    document::eval(
                        r#"const scroller = document.getElementById('channel-scroller');
                           if (scroller) { scroller.scrollTop = scroller.scrollHeight; }"#,
                    );
                } else if anchor_in_window {
                    if let Some(id) = scroll_anchor {
                        document::eval(&format!(
                            r#"const el = document.getElementById('msg-anchor-{id}');
                               if (el) {{ el.scrollIntoView({{ block: 'center', behavior: 'instant' }}); }}"#
                        ));
                    }
                } else if show_banner {
                    // Cursor older than the window / empty — land on the divider.
                    document::eval(
                        r#"const el = document.getElementById('new-messages-marker');
                           if (el) { el.scrollIntoView({ block: 'center', behavior: 'instant' }); }"#,
                    );
                }
                if let Some(mut session) = read_session() {
                    session.scrolled = true;
                    read_session.set(Some(session));
                }
                // Only auto-clear when the thread cannot scroll the break away
                // (short unread that already fits on screen).
                if show_banner {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if channel_unread_is_caught_up().await {
                        dismiss_channel_new_banner(read_session, refresh);
                    }
                }
            });
        } else if let Some(mut session) = read_session() {
            let unread = channel.unread_count.max(0) as usize;
            if session.show_banner {
                // Keep the divider count in sync as more traffic arrives.
                if unread == 0 {
                    session.show_banner = false;
                    session.new_count = 0;
                    read_session.set(Some(session));
                } else if unread != session.new_count {
                    session.new_count = unread;
                    read_session.set(Some(session));
                }
            } else if unread > 0 {
                // Channel was open and caught up — new traffic raises the
                // divider + badge until the operator scrolls past it.
                let stored_read = state().db.channel_last_read(slot).ok().flatten();
                read_session.set(Some(ChannelReadSession {
                    name: selected_name.clone(),
                    slot,
                    anchor_id: stored_read,
                    show_banner: true,
                    new_count: unread,
                    scrolled: true,
                }));
            }
        }
        let session = read_session().unwrap_or(ChannelReadSession {
            name: selected_name.clone(),
            slot,
            anchor_id: None,
            show_banner: false,
            new_count: 0,
            scrolled: true,
        });
        let bump_refresh = move || {
            spawn(async move {
                for _ in 0..8 {
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    refresh.set(refresh() + 1);
                }
            });
        };
        // One blocked-set load per paint — never resolve roster per message (that froze hover).
        let blocked_set: std::collections::HashSet<String> = state()
            .db
            .list_blocked_nodes()
            .unwrap_or_default()
            .into_iter()
            .map(|row| row.pubkey)
            .collect();
        let channel_appearance = state()
            .db
            .resolved_appearance(&channel.name)
            .unwrap_or_default();
        let channel_pane_style = channel_appearance.pane_style();
        let channel_pane_class = if channel_appearance.bg_opacity > 0 {
            "min-h-0 flex-1 space-y-3 overflow-auto p-4 messages-pane-themed message-pane-boost-bubbles"
        } else if channel_appearance.themed() {
            "min-h-0 flex-1 space-y-3 overflow-auto p-4 messages-pane-themed"
        } else {
            "min-h-0 flex-1 space-y-3 overflow-auto p-4"
        };
        let appear_channel_name = channel.name.clone();
        return rsx! {
            MessagesThreadShell {
                channels: channels.clone(),
                dm_threads: dm_threads.clone(),
                selected_channel: Some(channel.name.clone()),
                selected_dm: None,
                linked,
                rail_epoch,
                on_select_channel: move |name: String| {
                    selected_dm.set(None);
                    selected_channel.set(Some(name));
                    draft.set(String::new());
                    reply_target.set(None);
                    read_session.set(None);
                    msg_window.set(None);
                    note.set(String::new());
                },
                on_select_dm: move |pubkey: String| {
                    selected_channel.set(None);
                    selected_dm.set(Some(pubkey));
                    draft.set(String::new());
                    reply_target.set(None);
                    read_session.set(None);
                    msg_window.set(None);
                    note.set(String::new());
                },
                on_after_delete_channel: move |name: String| {
                    if selected_channel() == Some(name) {
                        selected_channel.set(None);
                        reply_target.set(None);
                        read_session.set(None);
                        msg_window.set(None);
                        draft.set(String::new());
                    }
                    refresh.set(refresh() + 1);
                },
                on_after_clear_dm: move |pubkey: String| {
                    if selected_dm() == Some(pubkey) {
                        selected_dm.set(None);
                        msg_window.set(None);
                        draft.set(String::new());
                    }
                    refresh.set(refresh() + 1);
                },
                on_channel_appearance: move |name: String| {
                    appearance_edit.set(Some(AppearanceEditTarget::Channel(name)));
                },
                AppearanceEditor {
                    target: appearance_edit,
                    on_saved: move |_| refresh.set(refresh() + 1),
                }
                div { class: "flex flex-wrap items-center justify-between gap-3",
                    div { class: "flex min-w-0 items-center gap-3",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| {
                                if let Some(name) = selected_channel() {
                                    if let Some(row) =
                                        state().db.list_channels().ok().and_then(|rows| {
                                            rows.into_iter().find(|row| row.name == name)
                                        })
                                    {
                                        let _ = state().db.mark_channel_caught_up(row.slot);
                                    }
                                }
                                selected_channel.set(None);
                                reply_target.set(None);
                                draft.set(String::new());
                                read_session.set(None);
                                msg_window.set(None);
                            },
                            Icon { name: IconName::ChevronRight, size: 14 }
                            "Channels"
                        }
                        div { class: "min-w-0",
                            h3 { class: "truncate text-lg font-semibold", "{channel.name}" }
                            div { class: "mt-1 flex flex-wrap gap-2",
                                Badge {
                                    tone: if channel.visibility == "public" { "ok".to_string() } else { "pending".to_string() },
                                    if channel.visibility == "public" { "Public" } else { "Private" }
                                }
                                if emergency {
                                    Badge { tone: "signal".to_string(), "HQ emergency" }
                                }
                                Badge { tone: "muted".to_string(), "Slot {channel.slot}" }
                            }
                        }
                    }
                    div { class: "channel-header-actions",
                        ChannelKeepSelect {
                            channel_name: channel.name.clone(),
                            current: channel
                                .message_keep
                                .clone()
                                .unwrap_or_else(|| "default".into()),
                            on_done: move |msg: String| {
                                // Keep changes belong in the status bar, not the chat header.
                                status_note(msg);
                            },
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| {
                                appearance_edit.set(Some(AppearanceEditTarget::Channel(
                                    appear_channel_name.clone(),
                                )));
                            },
                            Icon { name: IconName::Settings, size: 14 }
                            "Look"
                        }
                    }
                }
                div {
                    class: if emergency {
                        "flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border border-signal/40 bg-accent/40"
                    } else {
                        "flex min-h-0 flex-1 flex-col overflow-hidden rounded-lg border border-border bg-card"
                    },
                    div {
                        id: "channel-scroller",
                        // Remount the scroller when switching channels so a
                        // prior empty paint cannot stick in the WebView.
                        key: "channel-scroller-{selected_name}",
                        class: "{channel_pane_class}",
                        style: "{channel_pane_style}",
                        onscroll: move |_| {
                            if read_session().as_ref().is_some_and(|session| session.show_banner) {
                                spawn(async move {
                                    if channel_unread_is_caught_up().await {
                                        dismiss_channel_new_banner(read_session, refresh);
                                    }
                                });
                            }
                            if !has_older || loading_older() {
                                return;
                            }
                            let expect_name = selected_name.clone();
                            let load_slot = slot;
                            spawn(async move {
                                let near_top = document::eval(
                                    r#"return (() => {
                                      const s = document.getElementById('channel-scroller');
                                      return !!(s && s.scrollTop < 80);
                                    })();"#,
                                )
                                .await
                                .ok()
                                .and_then(|v| v.as_bool())
                                    == Some(true);
                                if !near_top || loading_older() {
                                    return;
                                }
                                let Some(window) = msg_window() else { return };
                                match &window.key {
                                    MsgWindowKey::Channel(name) if name == &expect_name => {}
                                    _ => return,
                                }
                                let Some(before_id) = window.messages.first().map(|m| m.id) else {
                                    return;
                                };
                                let prev_height = document::eval(
                                    r#"return document.getElementById('channel-scroller')?.scrollHeight ?? 0;"#,
                                )
                                .await
                                .ok()
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0);
                                loading_older.set(true);
                                match state().db.list_channel_messages_before(
                                    load_slot,
                                    before_id,
                                    MSG_WINDOW_PAGE,
                                ) {
                                    Ok((older, still_older)) if !older.is_empty() => {
                                        let mut next = window;
                                        let mut combined = older;
                                        combined.append(&mut next.messages);
                                        next.messages = combined;
                                        next.has_older = still_older;
                                        scroll_preserve_height.set(Some(prev_height));
                                        msg_window.set(Some(next));
                                    }
                                    Ok(_) => {
                                        if let Some(mut next) = msg_window() {
                                            next.has_older = false;
                                            msg_window.set(Some(next));
                                        }
                                    }
                                    Err(_) => {}
                                }
                                loading_older.set(false);
                            });
                        },
                        if has_older {
                            p { class: "text-center text-[0.7rem] text-muted-foreground",
                                if loading_older() { "Loading earlier messages…" } else { "Scroll up for earlier messages" }
                            }
                        }
                        if messages.is_empty() {
                            p { class: "text-sm text-muted-foreground", "No messages in this channel yet." }
                        }
                        {
                            // Place the divider after the last-read message. If the cursor is
                            // 0, missing, or older than the loaded window, put it above the
                            // first visible row (those rows are all unread relative to cursor).
                            let (banner_before_first, banner_after_id) = if !session.show_banner {
                                (false, None)
                            } else if session.anchor_id == Some(0) || session.anchor_id.is_none() {
                                (true, None::<i64>)
                            } else if let Some(aid) = session.anchor_id {
                                if messages.iter().any(|m| m.id == aid) {
                                    (false, Some(aid))
                                } else if let Some(prev) =
                                    messages.iter().rev().find(|m| m.id <= aid)
                                {
                                    (false, Some(prev.id))
                                } else {
                                    (true, None)
                                }
                            } else {
                                (true, None)
                            };
                            let new_count = session.new_count;
                            rsx! {
                        if banner_before_first {
                            div {
                                id: "new-messages-marker",
                                class: "my-3 flex w-full items-center gap-0 px-2",
                                div {
                                    class: "h-px min-w-0 flex-1",
                                    style: "background-color: rgba(61, 214, 140, 0.45);",
                                }
                                span {
                                    class: "shrink-0 text-[0.7rem] font-medium tracking-wide",
                                    style: "color: #3dd68c; background-color: #101418; padding: 0.35rem 0.85rem; margin: 0 0.15rem;",
                                    if new_count == 1 {
                                        "New messages · 1"
                                    } else {
                                        "New messages · {new_count}"
                                    }
                                }
                                div {
                                    class: "h-px min-w-0 flex-1",
                                    style: "background-color: rgba(61, 214, 140, 0.45);",
                                }
                            }
                        }
                        for message in messages {
                            {
                                let outgoing = message.direction == "tx";
                                let show_new_banner = banner_after_id == Some(message.id);
                                let new_count = new_count;
                                let (parsed_name, body) = channel_sender_and_body(&message.text);
                                let has_parsed_name = parsed_name
                                    .as_ref()
                                    .is_some_and(|name| !looks_like_channel_slot(name));
                                let device_name = parsed_name
                                    .filter(|name| !looks_like_channel_slot(name))
                                    .or_else(|| {
                                        let name = message.sender_name.trim();
                                        if name.is_empty() || looks_like_channel_slot(name) {
                                            None
                                        } else {
                                            Some(name.to_string())
                                        }
                                    })
                                    .unwrap_or_else(|| message.sender_name.clone());
                                let bubble_text = if has_parsed_name {
                                    body
                                } else {
                                    message.text.clone()
                                };
                                let target = MessageReplyTarget {
                                    id: message.id,
                                    sender_name: device_name.clone(),
                                    text: bubble_text.clone(),
                                };
                                // Prefer stored pubkey; channel frames often only have a display name.
                                let known_pk = message.sender_pubkey.clone();
                                let name_block = crate::db::name_block_key(&device_name);
                                let already_blocked = known_pk
                                    .as_ref()
                                    .is_some_and(|pk| blocked_set.contains(pk))
                                    || name_block
                                        .as_ref()
                                        .is_some_and(|nk| blocked_set.contains(nk));
                                let block_label = device_name.clone();
                                let avatar_seed = known_pk
                                    .clone()
                                    .filter(|pk| !pk.is_empty())
                                    .unwrap_or_else(|| device_name.clone());
                                let sender_tone = conversation_avatar_tone(&avatar_seed);
                                rsx! {
                                    div {
                                        id: "msg-anchor-{message.id}",
                                        key: "{message.id}",
                                        class: "flex w-full flex-col gap-1",
                                        div {
                                            class: if outgoing {
                                                "ml-auto flex max-w-[85%] flex-col items-end gap-1"
                                            } else {
                                                "mr-auto flex max-w-[85%] flex-col items-start gap-1"
                                            },
                                            p { class: "message-meta-time",
                                                "{format_time(Some(message.received_at))}"
                                            }
                                            div {
                                                class: if outgoing {
                                                    "message-bubble min-w-[14rem] rounded-2xl rounded-br-md border border-sky-500/30 bg-[#16324f] text-sm text-sky-50"
                                                } else if emergency {
                                                    "message-bubble min-w-[14rem] rounded-2xl rounded-bl-md border border-signal/30 bg-background/80 text-sm"
                                                } else {
                                                    "message-bubble min-w-[14rem] rounded-2xl rounded-bl-md message-bubble-tone-{sender_tone} text-sm"
                                                },
                                            div { class: "message-meta-head",
                                                if !outgoing {
                                                    ConversationAvatar {
                                                        label: device_name.clone(),
                                                        seed: avatar_seed.clone(),
                                                        large: true,
                                                    }
                                                }
                                                span { class: "message-meta-name", title: "{device_name}",
                                                    "{device_name}"
                                                }
                                            }
                                            MessageBody {
                                                text: bubble_text.clone(),
                                                outgoing,
                                            }
                                            div {
                                                class: if outgoing {
                                                    "message-action-wrap message-action-wrap-end"
                                                } else {
                                                    "message-action-wrap message-action-wrap-start"
                                                },
                                                div { class: "message-action-bar",
                                                    button {
                                                        class: "message-action-btn message-action-btn-reply",
                                                        title: "Reply",
                                                        onclick: {
                                                            let target = target.clone();
                                                            move |_| {
                                                                reply_target.set(Some(MessageReplyTarget {
                                                                    id: target.id,
                                                                    sender_name: target.sender_name.clone(),
                                                                    text: String::new(),
                                                                }));
                                                                note.set(String::new());
                                                            }
                                                        },
                                                        Icon { name: IconName::Reply, size: 12 }
                                                        "Reply"
                                                    }
                                                    button {
                                                        class: "message-action-btn message-action-btn-quote",
                                                        title: "Reply with quote",
                                                        onclick: {
                                                            let target = target.clone();
                                                            move |_| {
                                                                reply_target.set(Some(target.clone()));
                                                                note.set(String::new());
                                                            }
                                                        },
                                                        Icon { name: IconName::Quote, size: 12 }
                                                        "Quote"
                                                    }
                                                    button {
                                                        class: "message-action-btn message-action-btn-copy",
                                                        title: "Copy message",
                                                        onclick: {
                                                            let text = message.text.clone();
                                                            move |_| {
                                                                let escaped = text
                                                                    .replace('\\', "\\\\")
                                                                    .replace('`', "\\`")
                                                                    .replace('$', "\\$");
                                                                let script = format!(
                                                                    "navigator.clipboard.writeText(`{escaped}`).catch(() => {{}})"
                                                                );
                                                                document::eval(&script);
                                                                note.set("Copied message text.".into());
                                                            }
                                                        },
                                                        Icon { name: IconName::Copy, size: 12 }
                                                        "Copy"
                                                    }
                                                    if !outgoing {
                                                        {
                                                            let resolve_name = device_name.clone();
                                                            let known = known_pk.clone();
                                                            rsx! {
                                                                button {
                                                                    class: "message-action-btn message-action-btn-dm",
                                                                    title: "Open a direct message with this contact",
                                                                    onclick: {
                                                                        let resolve_name = resolve_name.clone();
                                                                        let known = known.clone();
                                                                        move |_| {
                                                                            match messages_rail::resolve_dm_pubkey(
                                                                                known.as_deref(),
                                                                                &resolve_name,
                                                                            ) {
                                                                                Ok(pubkey) => {
                                                                                    reply_target.set(None);
                                                                                    read_session.set(None);
                                                                                    msg_window.set(None);
                                                                                    draft.set(String::new());
                                                                                    note.set(String::new());
                                                                                    // Set DM first so the next paint lands on the
                                                                                    // DM branch (it is checked before channel).
                                                                                    selected_dm.set(Some(pubkey));
                                                                                    selected_channel.set(None);
                                                                                    rail_epoch.with_mut(|n| {
                                                                                        *n = n.wrapping_add(1)
                                                                                    });
                                                                                    refresh.set(refresh() + 1);
                                                                                }
                                                                                Err(err) => {
                                                                                    crate::toast::push(
                                                                                        crate::toast::ToastKind::Warn,
                                                                                        err,
                                                                                    );
                                                                                }
                                                                            }
                                                                        }
                                                                    },
                                                                    Icon { name: IconName::Messages, size: 12 }
                                                                    "DM"
                                                                }
                                                                button {
                                                                    class: "message-action-btn",
                                                                    title: "Favorite this contact",
                                                                    onclick: {
                                                                        let resolve_name = resolve_name.clone();
                                                                        let known = known.clone();
                                                                        move |_| {
                                                                            match messages_rail::toggle_contact_favorite_from_bubble(
                                                                                known.as_deref(),
                                                                                &resolve_name,
                                                                            ) {
                                                                                Ok((on, name_only)) => {
                                                                                    let msg = if on && name_only {
                                                                                        "Saved to Contacts → Pinned (channel name only — upgrades when they advert)."
                                                                                    } else if on {
                                                                                        "Contact added to Favorites (Contacts → Pinned)."
                                                                                    } else {
                                                                                        "Contact removed from Favorites."
                                                                                    };
                                                                                    status_note(msg);
                                                                                    rail_epoch.with_mut(|n| {
                                                                                        *n = n.wrapping_add(1)
                                                                                    });
                                                                                    refresh.set(refresh() + 1);
                                                                                }
                                                                                Err(err) => status_note(err),
                                                                            }
                                                                        }
                                                                    },
                                                                    Icon { name: IconName::Star, size: 12 }
                                                                    "Favorite"
                                                                }
                                                                button {
                                                                    class: "message-action-btn message-action-btn-block",
                                                                    title: if already_blocked {
                                                                        "Already blocked"
                                                                    } else {
                                                                        "Block this contact — drop future messages"
                                                                    },
                                                                    disabled: already_blocked,
                                                                    onclick: move |_| {
                                                                        let pubkey = known.clone().or_else(|| {
                                                                            state()
                                                                                .db
                                                                                .unique_node_by_display_name(&resolve_name)
                                                                                .ok()
                                                                                .flatten()
                                                                                .map(|n| n.pubkey)
                                                                        });
                                                                        let key = pubkey.or_else(|| {
                                                                            crate::db::name_block_key(&resolve_name)
                                                                        });
                                                                        let Some(key) = key else {
                                                                            block_resolve_fail.set(Some(
                                                                                resolve_name.clone(),
                                                                            ));
                                                                            return;
                                                                        };
                                                                        block_confirm.set(Some((
                                                                            key,
                                                                            block_label.clone(),
                                                                        )));
                                                                    },
                                                                    Icon { name: IconName::Ban, size: 12 }
                                                                    if already_blocked { "Blocked" } else { "Block" }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            }
                                            div { class: "message-meta-foot",
                                                if outgoing {
                                                    {
                                                        let (label, icon, pill) = match message.delivery.as_str() {
                                                            "queued" => ("Queued", IconName::Clock, "message-meta-pill message-meta-pill-ok"),
                                                            "sending" => ("Sending…", IconName::Clock, "message-meta-pill message-meta-pill-ok"),
                                                            "failed" => ("Not sent", IconName::Ban, "message-meta-pill message-meta-pill-bad"),
                                                            _ => ("Sent", IconName::Check, "message-meta-pill message-meta-pill-ok"),
                                                        };
                                                        rsx! {
                                                            span {
                                                                class: "{pill}",
                                                                title: if message.delivery == "failed" {
                                                                    "The companion did not accept this message. Check the status bar, then send it again."
                                                                } else {
                                                                    ""
                                                                },
                                                                Icon { name: icon, size: 11 }
                                                                "{label}"
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    span { class: "message-meta-pill",
                                                        Icon { name: IconName::Signal, size: 11 }
                                                        if let Some(hops) = message.hop_count {
                                                            "{hops} hops"
                                                        } else {
                                                            "hops —"
                                                        }
                                                    }
                                                    span { class: "message-meta-pill", "Received" }
                                                }
                                            }
                                        }
                                    }
                                    if show_new_banner {
                                        div {
                                            id: "new-messages-marker",
                                            class: "my-3 flex w-full items-center gap-0 px-2",
                                            div {
                                                class: "h-px min-w-0 flex-1",
                                                style: "background-color: rgba(61, 214, 140, 0.45);",
                                            }
                                            span {
                                                class: "shrink-0 text-[0.7rem] font-medium tracking-wide",
                                                style: "color: #3dd68c; background-color: #101418; padding: 0.35rem 0.85rem; margin: 0 0.15rem;",
                                                if new_count == 1 {
                                                    "New messages · 1"
                                                } else {
                                                    "New messages · {new_count}"
                                                }
                                            }
                                            div {
                                                class: "h-px min-w-0 flex-1",
                                                style: "background-color: rgba(61, 214, 140, 0.45);",
                                            }
                                        }
                                    }
                                }
                            }
                        }
                            }
                        }
                    }
                    div { class: "border-t border-border p-3",
                        if let Some(target) = reply_target() {
                            div { class: "mb-2 flex items-start gap-2 rounded-md border border-border bg-background/80 px-3 py-2",
                                Icon {
                                    name: if target.text.is_empty() { IconName::Reply } else { IconName::Quote },
                                    size: 14,
                                }
                                div { class: "min-w-0 flex-1",
                                    p { class: "text-[0.65rem] uppercase tracking-[0.14em] text-muted-foreground",
                                        if target.text.is_empty() {
                                            "Replying to {target.sender_name}"
                                        } else {
                                            "Quoting {target.sender_name}"
                                        }
                                    }
                                    if !target.text.is_empty() {
                                        p { class: "mt-1 line-clamp-2 text-xs text-muted-foreground",
                                            "{truncate_chars(&target.text, 120)}"
                                        }
                                    }
                                }
                                button {
                                    class: "inline-flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground cursor-pointer",
                                    title: "Cancel reply",
                                    onclick: move |_| reply_target.set(None),
                                    Icon { name: IconName::Close, size: 14 }
                                }
                            }
                        }
                        div {
                            class: "flex gap-2",
                            ComposePlusButton {
                                disabled: !linked || sending(),
                                show_quick_dm: true,
                                on_action: {
                                    let channel_name = selected_name.clone();
                                    move |action| {
                                        apply_compose_plus_action(
                                            action,
                                            draft,
                                            note,
                                            plus_dialog,
                                            plus_pick_query,
                                            plus_pick_value,
                                            plus_quick_body,
                                            Some(channel_name.clone()),
                                        );
                                    }
                                },
                            }
                            input {
                                class: "h-10 min-w-0 flex-1 rounded-md border border-border bg-background px-3 text-sm text-foreground outline-none",
                                value: draft(),
                                placeholder: if reply_target().is_some() {
                                    "Write your reply…"
                                } else {
                                    "Message this channel…"
                                },
                                oninput: move |event: FormEvent| {
                                    draft.set(clamp_mesh_text(event.value()));
                                },
                                onkeydown: move |event: KeyboardEvent| {
                                    if event.key() == Key::Enter {
                                        event.prevent_default();
                                        begin_channel_send(
                                            linked,
                                            slot,
                                            draft(),
                                            reply_target(),
                                            draft,
                                            reply_target,
                                            note,
                                            sending,
                                            bump_refresh,
                                        );
                                    }
                                },
                            }
                            EmojiPickerButton {
                                disabled: !linked,
                                align_end: true,
                                on_pick: move |emoji: String| {
                                    draft.set(append_emoji(&draft(), &emoji));
                                },
                            }
                            Button {
                                disabled: !linked || sending() || draft().trim().is_empty(),
                                onclick: move |_| {
                                    begin_channel_send(
                                        linked,
                                        slot,
                                        draft(),
                                        reply_target(),
                                        draft,
                                        reply_target,
                                        note,
                                        sending,
                                        bump_refresh,
                                    );
                                },
                                Icon { name: IconName::Messages, size: 14 }
                                if sending() { "Sending…" } else { "Send" }
                            }
                        }
                        div { class: "mt-2 flex flex-wrap items-center justify-between gap-2",
                            MeshTextCount { text: draft() }
                            if !linked {
                                p { class: "text-xs text-muted-foreground",
                                    if syncing {
                                        "Companion is still syncing — compose unlocks when Connected."
                                    } else {
                                        "Connect a companion to send on this channel."
                                    }
                                }
                            } else if !note().is_empty() {
                                p {
                                    class: if note().starts_with("Send failed") || note().starts_with("Not sent") {
                                        "text-xs text-red-300"
                                    } else if note() == "Sending…" {
                                        "text-xs text-muted-foreground"
                                    } else {
                                        "text-xs text-sky-200"
                                    },
                                    "{note}"
                                }
                            }
                        }
                    }
                }
                BlockContactDialog {
                    target: block_confirm,
                    on_blocked: move |_| refresh.set(refresh() + 1),
                }
                BlockResolveFailDialog { target: block_resolve_fail }
                ComposePlusDialogs {
                    dialog: plus_dialog,
                    pick_query: plus_pick_query,
                    pick_value: plus_pick_value,
                    quick_body: plus_quick_body,
                    draft,
                    note,
                    refresh,
                    selected_channel,
                    selected_dm,
                }
            }
        };
    }

    let messages_tab_now = messages_tab();
    let on_channels_tab = messages_tab_now == "channels";
    let on_direct_tab = messages_tab_now == "direct";
    let on_blocked_tab = messages_tab_now == "blocked";
    let blocked_nodes = state().db.list_blocked_nodes().unwrap_or_default();
    let blocked_count = blocked_nodes.len();
    let desk_title = match channel_desk_mode() {
        ChannelDeskMode::Pick => "Create / join channel",
        ChannelDeskMode::Create => "Create channel",
        ChannelDeskMode::JoinPrivate => "Join private channel",
        ChannelDeskMode::JoinPublic => "Restore Public",
        ChannelDeskMode::JoinHash => "Join hash channel",
    };
    let desk_subtitle = match channel_desk_mode() {
        ChannelDeskMode::Pick => "Choose what you want to do.".to_string(),
        ChannelDeskMode::Create => "Name a new channel on this headquarters.".to_string(),
        ChannelDeskMode::JoinPrivate => {
            "Add a channel you already have the 16-byte secret for.".to_string()
        }
        ChannelDeskMode::JoinPublic => {
            "Put the default Public channel back on slot 0.".to_string()
        }
        ChannelDeskMode::JoinHash => {
            "Secret is derived from the name (leading # optional). Not a bot command.".to_string()
        }
    };
    rsx! {
        // hq-main-fill is overflow:hidden for thread fill — this list must scroll.
        div { class: "min-h-0 flex-1 overflow-y-auto",
        div { class: "mx-auto flex max-w-6xl flex-col gap-4 pb-8",
            PageHeading { icon: IconName::Messages, title: "Messages".to_string(),
                "Channels on the headquarters companion, plus direct message threads. Channel secrets are never shown here."
            }
            div { class: "settings-subtabs",
                button {
                    class: if on_channels_tab { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                    onclick: move |_| {
                        messages_tab.set("channels".into());
                        selected_dm.set(None);
                    },
                    span { class: "inline-flex items-center gap-2",
                        "Channels"
                        if channel_unread_total > 0 {
                            span { class: "{unread_badge_class(channel_mention_total)}",
                                if channel_unread_total > 99 { "99+" } else { "{channel_unread_total}" }
                            }
                        }
                    }
                }
                button {
                    class: if on_direct_tab { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                    onclick: move |_| {
                        messages_tab.set("direct".into());
                        selected_channel.set(None);
                    },
                    span { class: "inline-flex items-center gap-2",
                        "Direct"
                        if dm_unread_total > 0 {
                            span { class: "{unread_badge_class(dm_mention_total)}",
                                if dm_unread_total > 99 { "99+" } else { "{dm_unread_total}" }
                            }
                        }
                    }
                }
                button {
                    class: if on_blocked_tab { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                    onclick: move |_| {
                        messages_tab.set("blocked".into());
                        selected_channel.set(None);
                        selected_dm.set(None);
                    },
                    span { class: "inline-flex items-center gap-2",
                        "Blocked"
                        if blocked_count > 0 {
                            span { class: "channel-unread-badge",
                                if blocked_count > 99 { "99+" } else { "{blocked_count}" }
                            }
                        }
                    }
                }
            }
            if on_channels_tab {
            div { class: "flex flex-wrap items-center justify-between gap-3",
                p { class: "text-sm text-muted-foreground",
                    "Open a channel, or create / join a new one. Retention lives under Default keep."
                }
                div { class: "flex flex-wrap items-center gap-2",
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| {
                            channel_desk_open.set(false);
                            keep_desk_open.set(true);
                        },
                        Icon { name: IconName::Clock, size: 14 }
                        "Default keep"
                    }
                    Button {
                        onclick: move |_| {
                            keep_desk_open.set(false);
                            channel_desk_mode.set(ChannelDeskMode::Pick);
                            channel_desk_open.set(true);
                        },
                        Icon { name: IconName::Messages, size: 14 }
                        "Create / join channel"
                    }
                }
            }
            SlideDownPanel {
                open: keep_desk_open(),
                title: "Default keep".to_string(),
                subtitle: "System default for channels and DMs. Each channel can override this on its card or while open.".to_string(),
                icon: Some(IconName::Clock),
                on_close: move |_| keep_desk_open.set(false),
                footer: rsx! {
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| keep_desk_open.set(false),
                        "Close"
                    }
                },
                div { class: "flex flex-col gap-3",
                    p { class: "text-sm text-muted-foreground",
                        "Roster, secrets, and audit stay. Global pane colors live under Settings → Messages appearance."
                    }
                    MessageKeepButton {
                        current: message_keep(),
                        // Slide-down clips absolute menus — render choices inline.
                        inline: true,
                        onpick: move |value: String| {
                            let _ = state().db.set_message_keep(&value);
                            message_keep.set(value.clone());
                            if value != "forever" {
                                let _ = state().db.apply_message_keep();
                                keep_applied_at.set(crate::db::unix_now());
                            }
                        },
                    }
                }
            }
            SlideDownPanel {
                open: channel_desk_open(),
                title: desk_title.to_string(),
                subtitle: desk_subtitle,
                icon: Some(IconName::Messages),
                on_close: move |_| {
                    channel_desk_open.set(false);
                    channel_desk_mode.set(ChannelDeskMode::Pick);
                    create_advanced.set(false);
                },
                footer: rsx! {
                    if channel_desk_mode() != ChannelDeskMode::Pick {
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| channel_desk_mode.set(ChannelDeskMode::Pick),
                            "Back"
                        }
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| {
                            channel_desk_open.set(false);
                            channel_desk_mode.set(ChannelDeskMode::Pick);
                            create_advanced.set(false);
                        },
                        "Close"
                    }
                },
                match channel_desk_mode() {
                    ChannelDeskMode::Pick => rsx! {
                        div { class: "flex flex-col gap-2",
                            button {
                                r#type: "button",
                                class: "dm-contact-option channel-desk-option",
                                onclick: move |_| channel_desk_mode.set(ChannelDeskMode::Create),
                                span { class: "channel-desk-option-icon channel-desk-option-icon-create",
                                    Icon { name: IconName::Plus, size: 16 }
                                }
                                span { class: "channel-desk-option-copy",
                                    span { class: "font-medium", "Create channel" }
                                    span { class: "text-[0.7rem] text-muted-foreground",
                                        "New private or public channel on this HQ"
                                    }
                                }
                            }
                            button {
                                r#type: "button",
                                class: "dm-contact-option channel-desk-option",
                                onclick: move |_| channel_desk_mode.set(ChannelDeskMode::JoinPrivate),
                                span { class: "channel-desk-option-icon channel-desk-option-icon-private",
                                    Icon { name: IconName::Key, size: 16 }
                                }
                                span { class: "channel-desk-option-copy",
                                    span { class: "font-medium", "Join private channel" }
                                    span { class: "text-[0.7rem] text-muted-foreground",
                                        "Name + 32-character hex secret"
                                    }
                                }
                            }
                            button {
                                r#type: "button",
                                class: "dm-contact-option channel-desk-option",
                                onclick: move |_| channel_desk_mode.set(ChannelDeskMode::JoinPublic),
                                span { class: "channel-desk-option-icon channel-desk-option-icon-public",
                                    Icon { name: IconName::Radio, size: 16 }
                                }
                                span { class: "channel-desk-option-copy",
                                    span { class: "font-medium", "Restore Public" }
                                    span { class: "text-[0.7rem] text-muted-foreground",
                                        "Put Public back on companion slot 0"
                                    }
                                }
                            }
                            button {
                                r#type: "button",
                                class: "dm-contact-option channel-desk-option",
                                onclick: move |_| channel_desk_mode.set(ChannelDeskMode::JoinHash),
                                span { class: "channel-desk-option-icon channel-desk-option-icon-hash",
                                    Icon { name: IconName::Signal, size: 16 }
                                }
                                span { class: "channel-desk-option-copy",
                                    span { class: "font-medium", "Join hash / hashtag channel" }
                                    span { class: "text-[0.7rem] text-muted-foreground",
                                        "Secret derived from the channel name"
                                    }
                                }
                            }
                        }
                    },
                    ChannelDeskMode::Create => rsx! {
                        div { class: "flex flex-col gap-3",
                            div { class: "min-w-0",
                                label {
                                    class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                    "Name"
                                }
                                TextInput {
                                    value: create_name(),
                                    placeholder: "Ops-Night".to_string(),
                                    kind: "text".to_string(),
                                    oninput: move |event: FormEvent| create_name.set(event.value()),
                                }
                            }
                            div { class: "min-w-0",
                                label {
                                    class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                    r#for: "channel-visibility",
                                    "Visibility"
                                }
                                SelectInput {
                                    id: "channel-visibility".to_string(),
                                    value: create_visibility(),
                                    onchange: move |event: FormEvent| create_visibility.set(event.value()),
                                    option { value: "private", style: "background-color: #151a20; color: #f4f4f5;", "Private" }
                                    option { value: "public", style: "background-color: #151a20; color: #f4f4f5;", "Public" }
                                }
                            }
                            button {
                                r#type: "button",
                                class: "self-start text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline cursor-pointer",
                                onclick: move |_| create_advanced.set(!create_advanced()),
                                if create_advanced() { "Hide advanced" } else { "Advanced: region (optional)" }
                            }
                            if create_advanced() {
                                {
                                    let regions = crate::mesh_regions::load_known_regions(&state().db);
                                    let active = crate::mesh_regions::load_active_region(&state().db)
                                        .unwrap_or_default();
                                    rsx! {
                                        div { class: "rounded-md border border-border bg-background/50 p-3",
                                            label {
                                                class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                                r#for: "channel-create-region",
                                                "Region"
                                            }
                                            p { class: "mb-2 text-xs text-muted-foreground",
                                                "Optional MeshCore region tag (Settings → Regions). Does not change the channel secret."
                                            }
                                            SelectInput {
                                                id: "channel-create-region".to_string(),
                                                value: create_region(),
                                                onchange: move |event: FormEvent| create_region.set(event.value()),
                                                option { value: "", style: "background-color: #151a20; color: #f4f4f5;", "None" }
                                                if !active.is_empty() {
                                                    option {
                                                        value: "{active}",
                                                        style: "background-color: #151a20; color: #f4f4f5;",
                                                        "#{active} (active)"
                                                    }
                                                }
                                                for name in regions.iter() {
                                                    if active.is_empty() || name != &active {
                                                        option {
                                                            value: "{name}",
                                                            style: "background-color: #151a20; color: #f4f4f5;",
                                                            "#{name}"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            Button {
                                onclick: move |_| {
                                    let region = if create_advanced() {
                                        Some(create_region())
                                    } else {
                                        None
                                    };
                                    let region_ref = region.as_deref().filter(|s| !s.trim().is_empty());
                                    match state().db.create_channel_with_region(
                                        &create_name(),
                                        &create_visibility(),
                                        &state().secrets,
                                        region_ref,
                                    ) {
                                        Ok(row) => {
                                            create_name.set(String::new());
                                            create_region.set(String::new());
                                            create_advanced.set(false);
                                            channel_desk_open.set(false);
                                            channel_desk_mode.set(ChannelDeskMode::Pick);
                                            let msg = format!("Created {}.", row.name);
                                            if linked {
                                                state().radio.push_channel(row.slot as u8, row.name.clone());
                                            }
                                            status_note(msg);
                                        }
                                        Err(err) => status_note(err.to_string()),
                                    }
                                },
                                Icon { name: IconName::Check, size: 14 }
                                "Create"
                            }
                        }
                    },
                    ChannelDeskMode::JoinPrivate => rsx! {
                        div { class: "flex flex-col gap-3",
                            TextInput {
                                value: join_private_name(),
                                placeholder: "Channel name".to_string(),
                                oninput: move |event: FormEvent| join_private_name.set(event.value()),
                            }
                            TextInput {
                                value: join_private_secret(),
                                placeholder: "32-char hex secret".to_string(),
                                oninput: move |event: FormEvent| join_private_secret.set(event.value()),
                            }
                            Button {
                                onclick: move |_| {
                                    match state().db.join_private_channel(
                                        &join_private_name(),
                                        &join_private_secret(),
                                        &state().secrets,
                                    ) {
                                        Ok(row) => {
                                            join_private_name.set(String::new());
                                            join_private_secret.set(String::new());
                                            channel_desk_open.set(false);
                                            channel_desk_mode.set(ChannelDeskMode::Pick);
                                            if linked {
                                                state().radio.push_channel(row.slot as u8, row.name.clone());
                                            }
                                            status_note(format!("Joined private channel {}.", row.name));
                                        }
                                        Err(err) => status_note(err.to_string()),
                                    }
                                },
                                "Join private"
                            }
                        }
                    },
                    ChannelDeskMode::JoinPublic => rsx! {
                        div { class: "flex flex-col gap-3",
                            p { class: "text-sm text-muted-foreground",
                                "Restores Public on mesh slot 0 if it was removed from this HQ list."
                            }
                            Button {
                                onclick: move |_| {
                                    match state().db.restore_public_channel() {
                                        Ok(row) => {
                                            channel_desk_open.set(false);
                                            channel_desk_mode.set(ChannelDeskMode::Pick);
                                            if linked {
                                                state().radio.push_channel(row.slot as u8, row.name.clone());
                                            }
                                            status_note("Public channel restored on slot 0.");
                                        }
                                        Err(err) => status_note(err.to_string()),
                                    }
                                },
                                Icon { name: IconName::Messages, size: 14 }
                                "Restore Public"
                            }
                        }
                    },
                    ChannelDeskMode::JoinHash => rsx! {
                        div { class: "flex flex-col gap-3",
                            TextInput {
                                value: join_hash_name(),
                                placeholder: "#community".to_string(),
                                oninput: move |event: FormEvent| join_hash_name.set(event.value()),
                            }
                            Button {
                                onclick: move |_| {
                                    match state().db.join_hash_channel(
                                        &join_hash_name(),
                                        &state().secrets,
                                    ) {
                                        Ok(row) => {
                                            join_hash_name.set(String::new());
                                            channel_desk_open.set(false);
                                            channel_desk_mode.set(ChannelDeskMode::Pick);
                                            if linked {
                                                state().radio.push_channel(row.slot as u8, row.name.clone());
                                            }
                                            status_note(format!("Joined hash channel {}.", row.name));
                                        }
                                        Err(err) => status_note(err.to_string()),
                                    }
                                },
                                "Join hash"
                            }
                        }
                    },
                }
            }
            div { class: "grid grid-cols-[repeat(auto-fill,minmax(16rem,1fr))] gap-3",
                for channel in channels {
                    {
                        let name = channel.name.clone();
                        let is_public_default = channel.name.eq_ignore_ascii_case("Public");
                        let is_system_private =
                            state().db.is_reserved_private(&channel.name);
                        let can_toggle_visibility = !is_public_default && !is_system_private;
                        let can_delete = !is_public_default && !is_system_private;
                        let system_private = is_system_private
                            || channel.role == "emergency"
                            || channel.role == "info"
                            || channel.role == "net";
                        let region_badge = channel.region.clone();
                        rsx! {
                            div {
                                key: "{channel.name}-{channel.slot}",
                                class: if system_private {
                                    "channel-card flex flex-col rounded-lg border border-signal/40 bg-accent/50 p-4"
                                } else {
                                    "channel-card flex flex-col rounded-lg border border-border bg-card p-4"
                                },
                                button {
                                    class: "flex min-w-0 flex-1 flex-col items-start gap-3 text-left cursor-pointer",
                                    onclick: move |_| {
                                        note.set(String::new());
                                        msg_window.set(None);
                                        selected_channel.set(Some(name.clone()));
                                    },
                                    div { class: "flex w-full min-w-0 items-start justify-between gap-2",
                                        div { class: "flex min-w-0 items-center gap-2",
                                            span {
                                                class: if system_private { "text-signal" } else { "text-muted-foreground" },
                                                Icon {
                                                    name: if system_private { IconName::Radio } else { IconName::Messages },
                                                    size: 18,
                                                }
                                            }
                                            p { class: "truncate font-semibold", "{channel.name}" }
                                            if channel.unread_count > 0 {
                                                span {
                                                    class: "{unread_badge_class(channel.unread_mentions)}",
                                                    if channel.unread_count > 99 {
                                                        "99+"
                                                    } else {
                                                        "{channel.unread_count}"
                                                    }
                                                }
                                            }
                                        }
                                        Badge {
                                            tone: if channel.visibility == "public" {
                                                "ok".to_string()
                                            } else {
                                                "pending".to_string()
                                            },
                                            if channel.visibility == "public" { "Public" } else { "Private" }
                                        }
                                    }
                                    if channel.role == "info" || channel.role == "emergency" {
                                        Badge { tone: "signal".to_string(), "Bot home" }
                                    } else if channel.role == "net" {
                                        Badge { tone: "signal".to_string(), "Peer net" }
                                    } else if channel.role == "hash" {
                                        Badge { tone: "pending".to_string(), "Hash" }
                                    }
                                    if let Some(region) = region_badge.as_ref() {
                                        Badge { tone: "pending".to_string(), "#{region}" }
                                    }
                                    p { class: "channel-card-preview",
                                        if let Some(text) = channel.last_text.as_ref() {
                                            "{text}"
                                        } else {
                                            "No messages yet"
                                        }
                                    }
                                    div { class: "mt-auto flex w-full items-center justify-between text-xs text-muted-foreground",
                                        span { "{channel.message_count} messages" }
                                        span { "Slot {channel.slot}" }
                                    }
                                }
                                div { class: "mt-3 flex flex-col gap-2 border-t border-border pt-3",
                                    ChannelKeepSelect {
                                        channel_name: name.clone(),
                                        current: channel
                                            .message_keep
                                            .clone()
                                            .unwrap_or_else(|| "default".into()),
                                        on_done: move |msg: String| note.set(msg),
                                    }
                                    if can_toggle_visibility || can_delete {
                                        div { class: "flex flex-wrap gap-2",
                                        if can_toggle_visibility {
                                            Button {
                                                kind: ButtonKind::Ghost,
                                                onclick: {
                                                    let name = name.clone();
                                                    let next = if channel.visibility == "public" {
                                                        "private".to_string()
                                                    } else {
                                                        "public".to_string()
                                                    };
                                                    move |_| {
                                                        match state().db.set_channel_visibility(&name, &next) {
                                                            Ok(()) => {
                                                                if linked {
                                                                    if let Some(row) = state()
                                                                        .db
                                                                        .list_channels()
                                                                        .ok()
                                                                        .and_then(|rows| {
                                                                            rows.into_iter().find(|row| row.name == name)
                                                                        })
                                                                    {
                                                                        state().radio.push_channel(
                                                                            row.slot as u8,
                                                                            row.name,
                                                                        );
                                                                    }
                                                                }
                                                                status_note(format!("{name} is now {next}."));
                                                            }
                                                            Err(err) => status_note(err.to_string()),
                                                        }
                                                    }
                                                },
                                                if channel.visibility == "public" { "Make private" } else { "Make public" }
                                            }
                                        }
                                        if can_delete {
                                            Button {
                                                kind: ButtonKind::Danger,
                                                onclick: {
                                                    let name = name.clone();
                                                    move |_| {
                                                        match state().db.soft_delete_channel(&name) {
                                                            Ok(slot) => {
                                                                if linked {
                                                                    status_note(format!(
                                                                        "Deleted {name}. Clearing companion slot {slot}…"
                                                                    ));
                                                                    let reply = state()
                                                                        .radio
                                                                        .clear_channel(slot as u8);
                                                                    let label = name.clone();
                                                                    spawn(async move {
                                                                        match reply.await {
                                                                            Ok(Ok(())) => status_note(format!(
                                                                                "Deleted {label} and cleared companion slot."
                                                                            )),
                                                                            Ok(Err(err)) => status_note(format!(
                                                                                "Deleted {label} locally, but companion clear failed: {err}"
                                                                            )),
                                                                            Err(_) => status_note(format!(
                                                                                "Deleted {label} locally; companion clear did not finish."
                                                                            )),
                                                                        }
                                                                    });
                                                                } else {
                                                                    status_note(format!(
                                                                        "Deleted {name}. Connect the companion to clear it from the radio too."
                                                                    ));
                                                                }
                                                            }
                                                            Err(err) => status_note(err.to_string()),
                                                        }
                                                    }
                                                },
                                                Icon { name: IconName::Close, size: 14 }
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
            }
            } else if on_direct_tab {
                {
                    let dm_contacts: Vec<(String, String)> = state()
                        .db
                        .list_nodes()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|node| node.enrollment != Enrollment::Hq)
                        .map(|node| {
                            let title = node_title(&node.name, &node.requested_name);
                            (node.pubkey, title)
                        })
                        .collect();
                    let default_dm_peer = dm_contacts
                        .first()
                        .map(|(pubkey, _)| pubkey.clone())
                        .unwrap_or_default();
                    let can_send_new = linked
                        && !new_dm_peer().is_empty()
                        && !new_dm_body().trim().is_empty();
                    rsx! {
                        div { class: "flex flex-wrap items-center justify-between gap-3",
                            p { class: "text-sm text-muted-foreground",
                                "Open a thread to reply, or start a new direct message."
                            }
                            Button {
                                onclick: move |_| {
                                    new_dm_body.set(String::new());
                                    new_dm_query.set(String::new());
                                    if new_dm_peer().is_empty() && !default_dm_peer.is_empty() {
                                        new_dm_peer.set(default_dm_peer.clone());
                                    }
                                    note.set(String::new());
                                    new_dm_open.set(true);
                                },
                                Icon { name: IconName::Messages, size: 14 }
                                "Send direct message"
                            }
                        }
                        if dm_threads.is_empty() {
                            Card { title: "Direct messages".to_string(), icon: Some(IconName::Messages),
                                p { class: "text-sm text-muted-foreground",
                                    "No threads yet. Use Send direct message, Message on Roster, or wait for a node to DM this companion."
                                }
                            }
                        } else {
                            div { class: "grid grid-cols-[repeat(auto-fill,minmax(16rem,1fr))] gap-3",
                                for thread in dm_threads {
                                    {
                                        let pubkey = thread.pubkey.clone();
                                        let open_key = pubkey.clone();
                                        rsx! {
                                            button {
                                                key: "{pubkey}",
                                                class: "flex flex-col rounded-lg border border-border bg-card p-4 text-left hover:bg-muted/40 cursor-pointer",
                                                onclick: move |_| {
                                                    note.set(String::new());
                                                    draft.set(String::new());
                                                    msg_window.set(None);
                                                    selected_dm.set(Some(open_key.clone()));
                                                },
                                                div { class: "flex w-full items-start justify-between gap-2",
                                                    div { class: "flex min-w-0 items-center gap-2",
                                                        span { class: "text-muted-foreground",
                                                            Icon { name: IconName::Messages, size: 18 }
                                                        }
                                                        p { class: "truncate font-semibold", "{thread.name}" }
                                                        if thread.unread_count > 0 {
                                                            span { class: "{unread_badge_class(thread.unread_mentions)}",
                                                                if thread.unread_count > 99 {
                                                                    "99+"
                                                                } else {
                                                                    "{thread.unread_count}"
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                                p { class: "mt-1 font-mono text-xs text-muted-foreground",
                                                    "{pubkey.chars().take(12).collect::<String>()}"
                                                }
                                                p { class: "mt-2 line-clamp-2 text-sm text-muted-foreground",
                                                    if let Some(text) = thread.last_text.as_ref() {
                                                        "{text}"
                                                    } else {
                                                        "No messages yet"
                                                    }
                                                }
                                                div { class: "mt-auto flex w-full items-center justify-between pt-3 text-xs text-muted-foreground",
                                                    span { "{thread.message_count} messages" }
                                                    span { "{format_time(thread.last_message_at)}" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        SlideDownPanel {
                            open: new_dm_open(),
                            title: "Send direct message".to_string(),
                            subtitle: "Pick a roster contact and write up to 160 bytes.".to_string(),
                            icon: Some(IconName::Messages),
                            on_close: move |_| {
                                new_dm_open.set(false);
                                new_dm_body.set(String::new());
                                new_dm_query.set(String::new());
                            },
                            footer: rsx! {
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| {
                                        new_dm_open.set(false);
                                        new_dm_body.set(String::new());
                                        new_dm_query.set(String::new());
                                    },
                                    "Cancel"
                                }
                                Button {
                                    disabled: !can_send_new,
                                    onclick: move |_| {
                                        let peer = new_dm_peer();
                                        let body = new_dm_body().trim().to_string();
                                        if peer.is_empty() || body.is_empty() {
                                            return;
                                        }
                                        if body.len() > MESH_TEXT_MAX_BYTES {
                                            note.set(format!(
                                                "Mesh text is limited to {MESH_TEXT_MAX_BYTES} bytes."
                                            ));
                                            return;
                                        }
                                        match state().db.queue_text(&peer, &body, &body) {
                                            Ok(_) => {
                                                new_dm_open.set(false);
                                                new_dm_body.set(String::new());
                                                new_dm_query.set(String::new());
                                                note.set(String::new());
                                                draft.set(String::new());
                                                selected_dm.set(Some(peer));
                                                refresh.set(refresh() + 1);
                                                status_note("Direct message queued for the companion.");
                                            }
                                            Err(err) => note.set(err.to_string()),
                                        }
                                    },
                                    Icon { name: IconName::Messages, size: 14 }
                                    "Send"
                                }
                            },
                            div { class: "flex flex-col gap-3",
                                if dm_contacts.is_empty() {
                                    p { class: "text-sm text-muted-foreground",
                                        "No roster contacts yet. Hear a node on the mesh first, then try again."
                                    }
                                } else {
                                    {
                                        let needle = new_dm_query();
                                        let mut ranked: Vec<(i32, String, String, String)> =
                                            dm_contacts
                                                .iter()
                                                .filter_map(|(pubkey, title)| {
                                                    let short =
                                                        pubkey.chars().take(12).collect::<String>();
                                                    let score = fuzzy_score(
                                                        &needle,
                                                        &[title.as_str(), pubkey.as_str(), short.as_str()],
                                                    )?;
                                                    Some((
                                                        score,
                                                        pubkey.clone(),
                                                        title.clone(),
                                                        short,
                                                    ))
                                                })
                                                .collect();
                                        ranked.sort_by(|a, b| {
                                            b.0.cmp(&a.0).then_with(|| a.2.cmp(&b.2))
                                        });
                                        ranked.truncate(16);
                                        let selected_label = dm_contacts
                                            .iter()
                                            .find(|(pubkey, _)| *pubkey == new_dm_peer())
                                            .map(|(pubkey, title)| {
                                                format!(
                                                    "{title} · {}",
                                                    pubkey.chars().take(12).collect::<String>()
                                                )
                                            });
                                        rsx! {
                                            div {
                                                label {
                                                    class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                                    "Contact"
                                                }
                                                TextInput {
                                                    value: new_dm_query(),
                                                    placeholder: "Fuzzy search name or pubkey…".to_string(),
                                                    kind: "text".to_string(),
                                                    oninput: move |event: FormEvent| {
                                                        new_dm_query.set(event.value());
                                                    },
                                                }
                                                if let Some(label) = selected_label {
                                                    p { class: "mt-1 text-xs text-signal",
                                                        "Selected: {label}"
                                                    }
                                                } else {
                                                    p { class: "mt-1 text-xs text-muted-foreground",
                                                        "Pick a contact from the matches below."
                                                    }
                                                }
                                                div { class: "dm-contact-results",
                                                    if ranked.is_empty() {
                                                        p { class: "px-3 py-3 text-sm text-muted-foreground",
                                                            "No roster matches for that search."
                                                        }
                                                    } else {
                                                        for (_score, pubkey, title, short) in ranked {
                                                            {
                                                                let pick = pubkey.clone();
                                                                let active = new_dm_peer() == pubkey;
                                                                rsx! {
                                                                    button {
                                                                        key: "{pubkey}",
                                                                        r#type: "button",
                                                                        class: if active {
                                                                            "dm-contact-option dm-contact-option-active"
                                                                        } else {
                                                                            "dm-contact-option"
                                                                        },
                                                                        onclick: move |_| {
                                                                            new_dm_peer.set(pick.clone());
                                                                        },
                                                                        span { class: "dm-contact-option-title", "{title}" }
                                                                        span { class: "dm-contact-option-key", "{short}" }
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
                                div {
                                    label {
                                        class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                        "Message"
                                    }
                                    div { class: "flex items-stretch gap-2",
                                        textarea {
                                            class: "min-h-28 min-w-0 flex-1 rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground outline-none",
                                            value: new_dm_body(),
                                            placeholder: "Write a direct message…",
                                            oninput: move |event: FormEvent| {
                                                new_dm_body.set(clamp_mesh_text(event.value()));
                                            },
                                        }
                                        div { class: "flex flex-col justify-end",
                                            EmojiPickerButton {
                                                disabled: !linked || dm_contacts.is_empty(),
                                                align_end: true,
                                                on_pick: move |emoji: String| {
                                                    new_dm_body.set(append_emoji(&new_dm_body(), &emoji));
                                                },
                                            }
                                        }
                                    }
                                    div { class: "mt-2 flex justify-end",
                                        MeshTextCount { text: new_dm_body() }
                                    }
                                }
                                if !linked {
                                    p { class: "text-xs text-muted-foreground",
                                        if syncing {
                                            "Companion is still syncing — compose unlocks when Connected."
                                        } else {
                                            "Connect a companion to send direct messages."
                                        }
                                    }
                                }
                                if !note().is_empty() {
                                    p { class: "text-xs text-amber-200", "{note}" }
                                }
                            }
                        }
                    }
                }
            } else {
                Card { title: "Blocked contacts".to_string(), icon: Some(IconName::Ban),
                    p { class: "mb-3 text-sm text-muted-foreground",
                        "Blocked senders are dropped on receive. Channel blocks can use a display name when the frame has no pubkey; DMs need a pubkey (promoted once HQ hears them). Enrollment is unchanged — unblock here to hear them again."
                    }
                    if blocked_nodes.is_empty() {
                        p { class: "text-sm text-muted-foreground",
                            "No blocked contacts. Use Block on an incoming channel message, or Block from an open DM."
                        }
                    } else {
                        div { class: "flex flex-col gap-2",
                            for row in blocked_nodes {
                                {
                                    let pubkey = row.pubkey.clone();
                                    let name_only = row.is_name_block();
                                    let short = if name_only {
                                        "channel name".to_string()
                                    } else {
                                        pubkey.chars().take(12).collect::<String>()
                                    };
                                    rsx! {
                                        div {
                                            key: "{pubkey}",
                                            class: "flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-background/60 px-3 py-2",
                                            div { class: "min-w-0",
                                                p { class: "truncate text-sm font-medium", "{row.label}" }
                                                p { class: if name_only { "text-xs text-muted-foreground" } else { "font-mono text-xs text-muted-foreground" }, "{short}" }
                                            }
                                            Button {
                                                kind: ButtonKind::Ghost,
                                                onclick: move |_| {
                                                    match state().db.unblock_node(&pubkey) {
                                                        Ok(()) => {
                                                            status_note("Contact unblocked.");
                                                            refresh.set(refresh() + 1);
                                                        }
                                                        Err(err) => status_note(err.to_string()),
                                                    }
                                                },
                                                "Unblock"
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

#[derive(Clone, PartialEq, Eq)]
struct MessageReplyTarget {
    id: i64,
    sender_name: String,
    /// Empty text means reply without quoting the body.
    text: String,
}

#[derive(Clone, PartialEq, Eq)]
struct ChannelReadSession {
    name: String,
    slot: i64,
    anchor_id: Option<i64>,
    show_banner: bool,
    new_count: usize,
    scrolled: bool,
}

async fn channel_unread_is_caught_up() -> bool {
    // Eval code runs as an async function body; the value must be returned.
    // Clear the New messages break once it reaches the top of the pane (scrolled
    // past), not an arbitrary mid-viewport fraction. If the whole thread fits
    // with no room to scroll the break away, treat visible unread as caught up.
    let result = document::eval(
        r#"return (() => {
          const scroller = document.getElementById('channel-scroller');
          if (!scroller) return false;
          const remaining =
            scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
          const marker = document.getElementById('new-messages-marker');
          if (!marker) return remaining < 56;
          const s = scroller.getBoundingClientRect();
          const m = marker.getBoundingClientRect();
          // Break has hit / cleared the top edge of the scroll pane.
          if (m.bottom <= s.top + 24) return true;
          const maxScroll = scroller.scrollHeight - scroller.clientHeight;
          if (maxScroll < 32) {
            // Nothing to scroll — new rows are already on screen.
            return m.bottom > s.top && m.top < s.bottom;
          }
          return false;
        })();"#,
    )
    .await;
    result.ok().and_then(|value| value.as_bool()) == Some(true)
}

fn dismiss_channel_new_banner(
    mut read_session: Signal<Option<ChannelReadSession>>,
    mut refresh: Signal<u64>,
) {
    let Some(mut session) = read_session() else {
        return;
    };
    if !session.show_banner {
        return;
    }
    let _ = state().db.mark_channel_caught_up(session.slot);
    session.show_banner = false;
    session.new_count = 0;
    read_session.set(Some(session));
    refresh.set(refresh() + 1);
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    let count = input.chars().count();
    if count <= max_chars {
        return input.to_string();
    }
    let mut out: String = input.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn looks_like_channel_slot(value: &str) -> bool {
    let trimmed = value.trim();
    let Some(rest) = trimmed
        .strip_prefix("ch")
        .or_else(|| trimmed.strip_prefix("CH"))
    else {
        return false;
    };
    !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit())
}

fn mention_device(name: &str) -> String {
    let name = name.trim().trim_matches(|ch| ch == '[' || ch == ']');
    if name.is_empty() || looks_like_channel_slot(name) {
        return String::new();
    }
    // MeshCore clients mention as @[name] even when the name has no spaces.
    format!("@[{name}]")
}

fn compose_channel_reply(target: Option<MessageReplyTarget>, text: &str) -> String {
    let text = text.trim();
    let Some(target) = target else {
        return text.to_string();
    };
    let mention = mention_device(&target.sender_name);
    if mention.is_empty() {
        return text.to_string();
    }
    if target.text.trim().is_empty() {
        return format!("{mention} {text}");
    }
    let quote = truncate_chars(target.text.trim(), 72);
    format!("> {mention} {quote}\n{text}")
}

fn begin_channel_send(
    linked: bool,
    slot: i64,
    raw: String,
    target: Option<MessageReplyTarget>,
    mut draft: Signal<String>,
    mut reply_target: Signal<Option<MessageReplyTarget>>,
    mut note: Signal<String>,
    mut sending: Signal<bool>,
    bump_refresh: impl Fn() + 'static,
) {
    let text = raw.trim().to_string();
    if text.is_empty() || sending() {
        return;
    }
    if !linked {
        note.set("Connect a companion radio before sending.".into());
        return;
    }
    let composed = compose_channel_reply(target, &text);
    let advert = state()
        .db
        .hq_advert_name()
        .unwrap_or_else(|_| "hq".into());
    let wire = prefix_channel_sender(&advert, &composed);
    if wire.len() > 160 {
        note.set("Mesh text is limited to 160 bytes after adding the device name.".into());
        return;
    }
    let slot_u8 = match u8::try_from(slot) {
        Ok(v) => v,
        Err(_) => {
            note.set(format!("Channel slot {slot} is out of range."));
            return;
        }
    };
    sending.set(true);
    note.set("Sending…".into());
    let reply = state().radio.send_channel(slot_u8, composed);
    spawn(async move {
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), reply).await;
        match outcome {
            Ok(Ok(Ok(()))) => {
                draft.set(String::new());
                reply_target.set(None);
                note.set(String::new());
                bump_refresh();
            }
            Ok(Ok(Err(err))) => note.set(err),
            Ok(Err(_)) => note.set("Radio task stopped before the send finished.".into()),
            Err(_) => note.set(
                "Send timed out waiting for the radio. Try Disconnect, then Connect, and send again."
                    .into(),
            ),
        }
        sending.set(false);
    });
}

#[component]
fn MessageBody(text: String, #[props(default)] outgoing: bool) -> Element {
    let parts = parse_message_parts(&text);
    let mention_style = if outgoing {
        "display:inline-flex;align-items:center;max-width:100%;margin:0 0.2rem;padding:0.2rem 0.55rem;border-radius:999px;border:1px solid rgba(125,211,252,0.4);background:rgba(56,189,248,0.16);color:#e0f2fe;font-size:0.75rem;font-weight:600;line-height:1.2;vertical-align:baseline;"
    } else {
        "display:inline-flex;align-items:center;max-width:100%;margin:0 0.2rem;padding:0.2rem 0.55rem;border-radius:999px;border:1px solid rgba(61,214,140,0.45);background:rgba(61,214,140,0.12);color:#3dd68c;font-size:0.75rem;font-weight:600;line-height:1.2;vertical-align:baseline;"
    };
    let link_class = if outgoing {
        "underline decoration-sky-200/70 underline-offset-2 hover:text-sky-100"
    } else {
        "underline decoration-signal/70 underline-offset-2 hover:text-signal"
    };
    let mesh_box = if outgoing {
        "my-1 flex max-w-full items-start gap-2 rounded-md border border-sky-400/30 bg-sky-950/40 px-2 py-1.5"
    } else {
        "my-1 flex max-w-full items-start gap-2 rounded-md border border-border bg-background/70 px-2 py-1.5"
    };
    let mesh_btn = if outgoing {
        "inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[0.65rem] text-sky-100 hover:bg-sky-400/15 cursor-pointer"
    } else {
        "inline-flex shrink-0 items-center gap-1 rounded px-1.5 py-0.5 text-[0.65rem] text-muted-foreground hover:bg-muted hover:text-foreground cursor-pointer"
    };
    rsx! {
        // `div` so meshcore:// chips can be block-level without invalid nesting.
        div { class: "message-body whitespace-pre-wrap break-words",
            for (idx, part) in parts.into_iter().enumerate() {
                {
                    match part {
                        TextPart::Text(value) => rsx! { span { key: "{idx}", "{value}" } },
                        TextPart::Mention(name) => rsx! {
                            span {
                                key: "{idx}",
                                class: "mention-pill",
                                style: "{mention_style}",
                                title: "@[{name}]",
                                "@{name}"
                            }
                        },
                        TextPart::Link(url) => rsx! {
                            a {
                                key: "{idx}",
                                class: "{link_class}",
                                href: "{url}",
                                target: "_blank",
                                rel: "noopener noreferrer",
                                "{url}"
                            }
                        },
                        TextPart::MeshLink(url) => {
                            let url_copy = url.clone();
                            let label = shorten_mesh_link(&url);
                            let contact = crate::contact_qr::parse_contact_add_uri(&url);
                            rsx! {
                                div {
                                    key: "{idx}",
                                    class: "{mesh_box}",
                                    title: "{url}",
                                    code {
                                        class: "min-w-0 flex-1 break-all font-mono text-[0.7rem] leading-snug text-foreground/90",
                                        "{label}"
                                    }
                                    if let Some((name, pubkey, _ty)) = contact {
                                        button {
                                            class: "{mesh_btn}",
                                            title: "Add this contact to Contacts",
                                            onclick: move |_| {
                                                match messages_rail::pin_contact_from_message(
                                                    &pubkey,
                                                    &name,
                                                ) {
                                                    Ok(_) => crate::toast::push(
                                                        crate::toast::ToastKind::Success,
                                                        format!("Added {name} to Contacts"),
                                                    ),
                                                    Err(err) => crate::toast::push(
                                                        crate::toast::ToastKind::Error,
                                                        err,
                                                    ),
                                                }
                                            },
                                            Icon { name: IconName::Users, size: 12 }
                                            "Add"
                                        }
                                    }
                                    button {
                                        class: "{mesh_btn}",
                                        title: "Copy meshcore link",
                                        onclick: move |_| {
                                            copy_text_to_clipboard(&url_copy);
                                            crate::toast::push(
                                                crate::toast::ToastKind::Success,
                                                "Copied meshcore link",
                                            );
                                        },
                                        Icon { name: IconName::Copy, size: 12 }
                                        "Copy"
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

fn copy_text_to_clipboard(text: &str) {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace('$', "\\$");
    let script = format!("navigator.clipboard.writeText(`{escaped}`).catch(() => {{}})");
    document::eval(&script);
}

/// Shorten long meshcore:// URIs for the bubble; full value stays in title + Copy.
fn shorten_mesh_link(url: &str) -> String {
    const KEEP_HEAD: usize = 22;
    const KEEP_TAIL: usize = 10;
    if url.len() <= KEEP_HEAD + KEEP_TAIL + 3 {
        return url.to_string();
    }
    format!("{}…{}", &url[..KEEP_HEAD], &url[url.len() - KEEP_TAIL..])
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TextPart {
    Text(String),
    Mention(String),
    /// http(s) — open in browser.
    Link(String),
    /// meshcore:// — copy chip (no shell / unconfigured URL open).
    MeshLink(String),
}

fn parse_message_parts(input: &str) -> Vec<TextPart> {
    let mut parts = Vec::new();
    for chunk in split_mentions(input) {
        match chunk {
            TextPart::Text(text) => parts.extend(linkify_text(&text)),
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        parts.push(TextPart::Text(String::new()));
    }
    parts
}

fn split_mentions(input: &str) -> Vec<TextPart> {
    let mut parts = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0usize;
    let mut buf = String::new();
    while i < chars.len() {
        if chars[i] == '@' {
            if i + 1 < chars.len() && chars[i + 1] == '[' {
                if let Some(end) = chars[i + 2..].iter().position(|ch| *ch == ']') {
                    let name: String = chars[i + 2..i + 2 + end].iter().collect();
                    let name = name.trim();
                    if !name.is_empty() {
                        if !buf.is_empty() {
                            parts.push(TextPart::Text(std::mem::take(&mut buf)));
                        }
                        parts.push(TextPart::Mention(name.to_string()));
                        i += 3 + end;
                        continue;
                    }
                }
            } else if i + 1 < chars.len() {
                let mut j = i + 1;
                while j < chars.len()
                    && (chars[j].is_ascii_alphanumeric()
                        || matches!(chars[j], '_' | '-' | '.'))
                {
                    j += 1;
                }
                if j > i + 1 {
                    let name: String = chars[i + 1..j].iter().collect();
                    if !buf.is_empty() {
                        parts.push(TextPart::Text(std::mem::take(&mut buf)));
                    }
                    parts.push(TextPart::Mention(name));
                    i = j;
                    continue;
                }
            }
        }
        buf.push(chars[i]);
        i += 1;
    }
    if !buf.is_empty() || parts.is_empty() {
        parts.push(TextPart::Text(buf));
    }
    parts
}

fn next_url_start(hay: &str) -> Option<(usize, &'static str)> {
    let candidates = ["https://", "http://", "meshcore://"];
    let mut best: Option<(usize, &'static str)> = None;
    for scheme in candidates {
        if let Some(pos) = hay.find(scheme) {
            if best.map(|(b, _)| pos < b).unwrap_or(true) {
                best = Some((pos, scheme));
            }
        }
    }
    best
}

fn linkify_text(input: &str) -> Vec<TextPart> {
    let mut parts = Vec::new();
    let mut rest = input;
    while let Some((start, scheme)) = next_url_start(rest) {
        if start > 0 {
            parts.push(TextPart::Text(rest[..start].to_string()));
        }
        let after = &rest[start..];
        let end = after
            .find(|ch: char| ch.is_whitespace() || matches!(ch, ')' | ']' | '"' | '\'' | '<' | '>'))
            .unwrap_or(after.len());
        let mut url = after[..end].to_string();
        while url.ends_with(['.', ',', ';', ':', '!', '?']) {
            url.pop();
        }
        if url.len() <= scheme.len() {
            parts.push(TextPart::Text(after[..end].to_string()));
        } else if scheme == "meshcore://" {
            parts.push(TextPart::MeshLink(url));
        } else {
            parts.push(TextPart::Link(url));
        }
        rest = &after[end..];
    }
    if !rest.is_empty() || parts.is_empty() {
        parts.push(TextPart::Text(rest.to_string()));
    }
    parts
}

#[cfg(test)]
mod message_body_tests {
    use super::{linkify_text, parse_message_parts, shorten_mesh_link, TextPart};

    #[test]
    fn linkifies_https_and_meshcore() {
        let parts = linkify_text(
            "see https://example.com/x and meshcore://aabbccddeeff00112233445566778899 here",
        );
        assert!(parts.iter().any(|p| matches!(p, TextPart::Link(u) if u.starts_with("https://"))));
        assert!(parts
            .iter()
            .any(|p| matches!(p, TextPart::MeshLink(u) if u.starts_with("meshcore://"))));
    }

    #[test]
    fn meshcore_only_message_is_one_chip() {
        let url = "meshcore://1100eeb86d8060b5063e823c520562e412b9c931c9c090bf39a851e759fce4201d1c8be9e86aa6dfece6b5f233613488861c849";
        let parts = parse_message_parts(url);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], TextPart::MeshLink(url.into()));
    }

    #[test]
    fn strips_trailing_punctuation_from_links() {
        let parts = linkify_text("go https://map.meshcore.dev.");
        assert_eq!(
            parts,
            vec![
                TextPart::Text("go ".into()),
                TextPart::Link("https://map.meshcore.dev".into()),
            ]
        );
    }

    #[test]
    fn shortens_long_mesh_links() {
        let url = "meshcore://1100eeb86d8060b5063e823c520562e412b9c931c9c090bf39a851e759fce4201d1c8be9e86aa6dfece6b5f233613488861c849";
        let short = shorten_mesh_link(url);
        assert!(short.contains('…'));
        assert!(short.starts_with("meshcore://"));
        assert!(short.ends_with("861c849") || short.len() < url.len());
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Hq,
    Device,
    Flasher,
    Diagnostics,
    About,
}

/// Heavy Flasher panel data — rebuilt only when the cache key changes (not every 350ms poll).
#[derive(Clone)]
struct FlasherPanelCache {
    key: String,
    filtered: Vec<crate::firmware::DeviceChoice>,
    selected_device: Option<crate::firmware::DeviceChoice>,
    selected_role: Option<crate::firmware::RoleChoice>,
    version_options: Vec<(String, String)>,
    ports: Vec<crate::usb_serial::UsbPort>,
    asset_ready: Option<(PathBuf, bool)>,
    catalog_missing: bool,
}

fn flasher_panel_cache() -> &'static Mutex<Option<FlasherPanelCache>> {
    static CACHE: OnceLock<Mutex<Option<FlasherPanelCache>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

fn load_flasher_panel_cache(
    key: String,
    selection: &crate::firmware::FirmwareSelection,
    device: &str,
    role: &str,
    tag: &str,
    erase: bool,
    query: &str,
) -> FlasherPanelCache {
    if let Ok(guard) = flasher_panel_cache().lock() {
        if let Some(cached) = guard.as_ref() {
            if cached.key == key {
                return cached.clone();
            }
        }
    }
    let index = crate::firmware::load_index();
    let catalog = crate::firmware::load_catalog().ok();
    let catalog_missing = catalog.is_none();
    let devices = catalog
        .as_ref()
        .map(crate::firmware::community_devices)
        .unwrap_or_default();
    let selected_keys: std::collections::BTreeSet<_> =
        selection.entries.iter().map(|e| e.key.clone()).collect();
    let query_l = query.trim().to_ascii_lowercase();
    let mut filtered: Vec<_> = devices
        .iter()
        .filter(|d| {
            query_l.is_empty()
                || d.name.to_ascii_lowercase().contains(&query_l)
                || d.maker.to_ascii_lowercase().contains(&query_l)
        })
        .cloned()
        .collect();
    filtered.sort_by_key(|d| {
        let preferred = d.roles.iter().any(|r| {
            selected_keys.contains(&crate::firmware::selection_key(&d.name, &r.role))
        });
        (!preferred, d.name.to_ascii_lowercase())
    });
    let selected_device = devices
        .iter()
        .find(|d| d.name == device)
        .cloned()
        .or_else(|| filtered.first().cloned());
    let selected_role = selected_device.as_ref().and_then(|d| {
        d.roles
            .iter()
            .find(|r| r.role == role)
            .cloned()
            .or_else(|| d.roles.first().cloned())
    });
    let version_options: Vec<(String, String)> = if let Some(role_c) = selected_role.as_ref() {
        if !role_c.static_versions.is_empty() {
            role_c
                .static_versions
                .iter()
                .map(|v| (v.version.clone(), v.version.clone()))
                .collect()
        } else {
            let key_kind = crate::firmware::flash_kind_key(erase, &role_c.patterns);
            let pattern = role_c.patterns.get(key_kind).cloned().unwrap_or_default();
            if pattern.is_empty() {
                Vec::new()
            } else {
                crate::firmware::local_versions_for_role(&index, &role_c.family, &pattern)
            }
        }
    } else {
        Vec::new()
    };
    let tag_effective = if !version_options.is_empty()
        && version_options.iter().all(|(t, _)| t != tag)
    {
        version_options[0].0.clone()
    } else {
        tag.to_string()
    };
    let ports = crate::firmware::list_usb_ports_for_flash();
    let asset_ready = selected_role.as_ref().and_then(|role_c| {
        crate::firmware::resolve_flash_asset(&index, role_c, &tag_effective, erase).ok()
    });
    let built = FlasherPanelCache {
        key,
        filtered,
        selected_device,
        selected_role,
        version_options,
        ports,
        asset_ready,
        catalog_missing,
    };
    if let Ok(mut guard) = flasher_panel_cache().lock() {
        *guard = Some(built.clone());
    }
    built
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeviceTab {
    Identity,
    Radio,
    Network,
    Advanced,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HqTab {
    /// Pair / reconnect companions.
    Connect,
    /// Date/time, secrets label, message keep / purge.
    Preferences,
    /// Weather, NWS, quake, Amber toggles.
    Services,
    /// Backup, contacts I/O, offline map packs.
    Data,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConnectLinkTab {
    Bluetooth,
    Usb,
    Network,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ServicesTab {
    Site,
    Weather,
    Ecowitt,
    Mqtt,
    LocalApi,
    Alerts,
    Quake,
}

fn custom_var_label(key: &str) -> String {
    let lower = key.to_ascii_lowercase();
    if lower.contains("gps") {
        format!("GPS ({key})")
    } else if lower.contains("display") || lower.contains("screen") || lower.contains("oled") {
        format!("Display ({key})")
    } else if lower.contains("sleep") {
        format!("Sleep ({key})")
    } else if lower.contains("power") || lower.contains("bat") {
        format!("Power ({key})")
    } else if lower.contains("advert") {
        format!("Advert ({key})")
    } else if lower.contains("path") && lower.contains("hash") {
        format!("Path hash ({key})")
    } else if lower.contains("loc") {
        format!("Location ({key})")
    } else {
        key.to_string()
    }
}

fn path_hash_var_key(vars: &std::collections::BTreeMap<String, String>) -> Option<String> {
    vars.keys()
        .find(|key| {
            let lower = key.to_ascii_lowercase();
            lower.contains("path.hash")
                || lower.contains("path_hash")
                || lower == "path.hash.mode"
                || lower == "hash_mode"
                || lower.ends_with(".hash_mode")
                || lower == "radio.hash_mode"
        })
        .cloned()
}

fn gps_enabled_var_key(vars: &std::collections::BTreeMap<String, String>) -> Option<String> {
    vars.keys()
        .find(|key| {
            let lower = key.to_ascii_lowercase();
            lower == "gps"
                || lower == "gps_enabled"
                || lower == "gps.en"
                || lower == "gps.enabled"
        })
        .cloned()
}

fn gps_interval_var_key(vars: &std::collections::BTreeMap<String, String>) -> Option<String> {
    vars.keys()
        .find(|key| {
            let lower = key.to_ascii_lowercase();
            lower == "gps_interval"
                || lower == "gps.int"
                || lower == "gps.interval"
        })
        .cloned()
}

fn share_loc_var_key(vars: &std::collections::BTreeMap<String, String>) -> Option<String> {
    vars.keys()
        .find(|key| {
            let lower = key.to_ascii_lowercase();
            lower == "gps.adv_loc"
                || lower == "adv_loc"
                || lower.contains("adv_loc")
                || lower == "advert_loc_policy"
        })
        .cloned()
}

/// Companion battery tradeoffs we can set over BLE (not repeater `powersaving`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum BatteryProfile {
    High,
    Balanced,
    Saver,
}

impl BatteryProfile {
    fn from_value(value: &str) -> Option<Self> {
        match value {
            "high" => Some(Self::High),
            "balanced" => Some(Self::Balanced),
            "saver" => Some(Self::Saver),
            _ => None,
        }
    }

    fn value(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Balanced => "balanced",
            Self::Saver => "saver",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::High => "High performance",
            Self::Balanced => "Balanced",
            Self::Saver => "Power saver",
        }
    }

    fn summary(self) -> &'static str {
        match self {
            Self::High => "Full TX, GPS on with short updates, share location in adverts.",
            Self::Balanced => "Moderate TX, GPS on with 5‑minute updates.",
            Self::Saver => "Lower TX, GPS off, no location in adverts.",
        }
    }
}

fn apply_battery_profile(profile: BatteryProfile, radio: &crate::radio::RadioStatus) -> String {
    let max_tx = radio.max_tx_power.unwrap_or(22).max(1);
    let (tx, gps_en, gps_interval, loc_policy) = match profile {
        BatteryProfile::High => (max_tx, "1", "30", 2u8),
        BatteryProfile::Balanced => (max_tx.min(14).max(1), "1", "300", 1u8),
        BatteryProfile::Saver => (max_tx.min(10).max(1), "0", "600", 0u8),
    };

    state().radio.set_tx_power(tx);
    state().radio.set_adv_loc_policy(loc_policy);

    let mut applied = vec![format!("TX {tx} dBm"), format!("advert loc {}", loc_policy_label(loc_policy))];
    if let Some(key) = gps_enabled_var_key(&radio.custom_vars) {
        state().radio.set_custom_var(key, gps_en.into());
        applied.push(if gps_en == "1" {
            "GPS on".into()
        } else {
            "GPS off".into()
        });
    }
    if let Some(key) = gps_interval_var_key(&radio.custom_vars) {
        state().radio.set_custom_var(key, gps_interval.into());
        applied.push(format!("GPS interval {gps_interval}s"));
    }
    if let Some(key) = share_loc_var_key(&radio.custom_vars) {
        // Some boards expose advert location as a custom var instead of policy.
        let value = match loc_policy {
            0 => "0",
            2 => "2",
            _ => "1",
        };
        state().radio.set_custom_var(key, value.into());
    }

    format!(
        "{} applied ({}). Disconnect when idle — BLE keeps the companion awake.",
        profile.label(),
        applied.join(", ")
    )
}

fn nearest_bw_option(bw: f64) -> f64 {
    BW_OPTIONS_KHZ
        .iter()
        .copied()
        .min_by(|a, b| {
            (a - bw)
                .abs()
                .partial_cmp(&(b - bw).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(62.5)
}

fn apply_path_hash_mode_ui(mode: u8) {
    let mode = mode.min(2);
    state().radio.set_path_hash_mode(mode);
    let vars = state().radio.snapshot().custom_vars;
    if let Some(key) = path_hash_var_key(&vars) {
        state().radio.set_custom_var(key, mode.to_string());
    }
}

fn is_boolish_var(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "on" | "true" | "yes" | "enable" | "enabled" => Some(true),
        "0" | "off" | "false" | "no" | "disable" | "disabled" => Some(false),
        _ => None,
    }
}

fn flip_boolish_var(value: &str) -> String {
    let trimmed = value.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "1" => "0".into(),
        "0" => "1".into(),
        "on" => "off".into(),
        "off" => "on".into(),
        "true" => "false".into(),
        "false" => "true".into(),
        "yes" => "no".into(),
        "no" => "yes".into(),
        "enable" | "enabled" => "off".into(),
        "disable" | "disabled" => "on".into(),
        _ => trimmed.to_string(),
    }
}

#[component]
fn SettingsPage(
    radio: RadioStatus,
    prefer_about: bool,
    on_about_consumed: EventHandler<()>,
    on_open_help: EventHandler<()>,
    on_request_pin: EventHandler<String>,
) -> Element {
    let saved_name = state().db.setting("radio_name").ok().flatten().unwrap_or_default();
    let saved_hq = state()
        .db
        .setting("hq_name")
        .ok()
        .flatten()
        .unwrap_or_else(|| "Metis HQ".into());
    let secret_label = state()
        .db
        .secret_store_label()
        .unwrap_or_else(|_| "Unknown".into());

    let saved_time = state()
        .db
        .setting("time_format")
        .ok()
        .flatten()
        .unwrap_or_else(|| TimeFormatPref::TwelveHour.as_str().into());
    let saved_date = state()
        .db
        .setting("date_format")
        .ok()
        .flatten()
        .unwrap_or_else(|| DateFormatPref::MonthDayShort.as_str().into());
    let mut radio_name = use_signal(|| saved_name);
    let mut tcp_host = use_signal(String::new);
    let mut tcp_port = use_signal(|| crate::tcp_radio::DEFAULT_TCP_PORT.to_string());
    let mut connect_link_tab = use_signal(|| ConnectLinkTab::Bluetooth);
    let mut hq_name = use_signal(|| saved_hq);
    let mut time_format = use_signal(|| saved_time);
    let mut date_format = use_signal(|| saved_date);
    let mut message_keep = use_signal(|| {
        state()
            .db
            .message_keep()
            .unwrap_or_else(|_| "forever".into())
    });
    let mut appearance_edit = use_signal(|| None::<AppearanceEditTarget>);
    let mut regions_tick = use_signal(|| 0u32);
    let mut confirm_purge = use_signal(|| false);
    let mut purge_note = use_signal(String::new);
    let mut confirm_infra_purge = use_signal(|| false);
    let mut infra_purge_note = use_signal(String::new);
    let mut infra_probe_interval = use_signal(|| {
        state().db.infra_probe_interval_mins().to_string()
    });
    let mut infra_probe_max = use_signal(|| {
        state().db.infra_probe_max_targets().to_string()
    });
    let mut infra_status_cap = use_signal(|| {
        state().db.infra_status_sample_cap().to_string()
    });
    let mut infra_status_keep = use_signal(|| {
        state()
            .db
            .infra_status_keep()
            .unwrap_or_else(|_| "forever".into())
    });
    let mut map_upload_enabled = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_map_upload_enabled", false)
    });
    let mut map_upload_authorized = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_map_upload_authorized", false)
    });
    let mut map_upload_dry_run = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_map_upload_dry_run", false)
    });
    let mut map_upload_interval = use_signal(|| {
        state()
            .db
            .setting("svc_map_upload_interval_secs")
            .ok()
            .flatten()
            .unwrap_or_else(|| "3600".into())
    });
    let mut map_upload_api = use_signal(|| {
        state()
            .db
            .setting("svc_map_upload_api_url")
            .ok()
            .flatten()
            .unwrap_or_else(|| "https://map.meshcore.io/api/v1/uploader/node".into())
    });
    let mut map_upload_note = use_signal(String::new);
    let mut map_upload_tick = use_signal(|| 0u32);
    let mut backup_pass = use_signal(String::new);
    let mut backup_pass2 = use_signal(String::new);
    let mut backup_note = use_signal(String::new);
    let mut backups_tick = use_signal(|| 0u32);
    let mut download_backups_open = use_signal(|| false);
    let mut backup_schedule = use_signal(|| {
        state()
            .db
            .setting("backup_schedule")
            .ok()
            .flatten()
            .unwrap_or_else(|| "off".into())
    });
    // Last schedule value successfully written to the DB (dropdown can differ until Save).
    let mut backup_schedule_saved = use_signal(|| backup_schedule());
    let mut restore_path = use_signal(String::new);
    let mut restore_pass = use_signal(String::new);
    let mut restore_preview = use_signal(|| None::<(String, String, crate::backup::BackupPreview)>);
    let mut contacts_note = use_signal(String::new);
    let mut exports_tick = use_signal(|| 0u32);
    let mut download_exports_open = use_signal(|| false);
    let mut contacts_import_path = use_signal(String::new);
    let mut confirm_contacts_import = use_signal(|| false);
    let mut contacts_import_preview = use_signal(String::new);
    let mut device_query = use_signal(String::new);
    let mut lat_draft = use_signal(String::new);
    let mut lon_draft = use_signal(String::new);
    let mut tx_draft = use_signal(String::new);
    let mut freq_draft = use_signal(String::new);
    let mut bw_draft = use_signal(String::new);
    let mut sf_draft = use_signal(String::new);
    let mut cr_draft = use_signal(String::new);
    let mut repeat_draft = use_signal(|| false);
    let mut region_select = use_signal(|| {
        crate::mesh_regions::load_active_region(&state().db).unwrap_or_default()
    });
    let mut region_manual = use_signal(String::new);
    let mut regions_list_tick = use_signal(|| 0u32);
    let mut region_scan_busy = use_signal(|| false);
    let mut region_note = use_signal(String::new);
    let mut node_discover_busy = use_signal(|| false);
    let mut node_discover_note = use_signal(String::new);
    let mut preset_open = use_signal(|| false);
    let mut preset_query = use_signal(String::new);
    let mut settings_tab = use_signal(|| SettingsTab::Hq);
    let mut about_applied = use_signal(|| false);
    if prefer_about && !about_applied() {
        settings_tab.set(SettingsTab::About);
        about_applied.set(true);
        on_about_consumed.call(());
    }
    let mut hq_tab = use_signal(|| HqTab::Connect);
    let mut services_tab = use_signal(|| ServicesTab::Site);
    let mut usb_scanned = use_signal(|| false);
    let mut flasher_tick = use_signal(|| 0u32);
    let mut flasher_pack_note = use_signal(String::new);
    let mut flasher_readiness =
        use_signal(|| Option::<crate::firmware::ReadinessReport>::None);
    let mut flasher_device = use_signal(String::new);
    let mut flasher_role = use_signal(String::new);
    let mut flasher_tag = use_signal(String::new);
    let mut flasher_port = use_signal(String::new);
    let mut flasher_erase = use_signal(|| false);
    let mut flasher_erase_armed = use_signal(|| false);
    let mut flasher_query = use_signal(String::new);
    let mut flasher_manage_open = use_signal(|| false);
    let mut flasher_manage_query = use_signal(String::new);
    let mut flasher_manage_draft =
        use_signal(std::collections::BTreeMap::<String, crate::firmware::SelectionEntry>::new);
    let mut flasher_manage_offers =
        use_signal(Vec::<crate::firmware::FirmwareOffer>::new);
    let mut flasher_manage_on_disk =
        use_signal(std::collections::BTreeSet::<String>::new);
    let mut flasher_manage_unavailable =
        use_signal(std::collections::BTreeSet::<String>::new);
    let mut flasher_manage_family_tags =
        use_signal(std::collections::BTreeMap::<String, Vec<String>>::new);
    // Expanded device cards in Manage (collapsed by default — full expand freezes WebView).
    let mut flasher_manage_expanded =
        use_signal(std::collections::BTreeSet::<String>::new);
    let mut flasher_download_all = use_signal(|| false);
    let mut flasher_sync_enabled = use_signal(|| {
        crate::services::setting_bool(&state().db, "firmware_sync_enabled", true)
    });
    let mut flasher_sync_interval = use_signal(|| {
        crate::firmware::sync_interval_hours(&state().db).to_string()
    });
    let mut device_tab = use_signal(|| DeviceTab::Identity);
    let mut var_drafts = use_signal(std::collections::BTreeMap::<String, String>::new);
    let mut reboot_armed = use_signal(|| false);
    let mut gps_interval_draft = use_signal(String::new);
    let mut battery_profile = use_signal(|| "balanced".to_string());
    let mut battery_note = use_signal(String::new);
    let mut log_filter = use_signal(|| "all".to_string());
    let mut log_query = use_signal(String::new);
    let mut log_autoscroll = use_signal(|| true);
    let mut svc_weather = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_weather_enabled", true)
    });
    let mut svc_weather_unit = use_signal(|| {
        state()
            .db
            .setting("svc_weather_unit")
            .ok()
            .flatten()
            .unwrap_or_else(|| "F".into())
    });
    let mut svc_nws = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_nws_enabled", false)
    });
    let mut svc_nws_auto = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_nws_auto_relay", false)
    });
    let mut svc_quake = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_quake_enabled", false)
    });
    let mut svc_quake_mag = use_signal(|| {
        state()
            .db
            .setting("svc_quake_min_mag")
            .ok()
            .flatten()
            .unwrap_or_else(|| "3.0".into())
    });
    let mut svc_site_lat = use_signal(|| {
        state()
            .db
            .setting("hq_site_lat")
            .ok()
            .flatten()
            .unwrap_or_default()
    });
    let mut svc_site_lon = use_signal(|| {
        state()
            .db
            .setting("hq_site_lon")
            .ok()
            .flatten()
            .unwrap_or_default()
    });
    let mut channel_info_name = use_signal(|| {
        state()
            .db
            .info_channel_name()
            .unwrap_or_else(|_| crate::secrets::CHANNEL_NAME.into())
    });
    let mut channel_net_name = use_signal(|| {
        state()
            .db
            .net_channel_name()
            .unwrap_or_else(|_| crate::secrets::NET_CHANNEL_NAME.into())
    });
    let mut svc_weather_hourly = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_weather_hourly", false)
    });
    let mut svc_weather_zips = use_signal(|| {
        state()
            .db
            .setting("svc_weather_zips")
            .ok()
            .flatten()
            .unwrap_or_default()
    });
    let mut svc_weather_interval = use_signal(|| {
        state()
            .db
            .setting("svc_weather_interval_mins")
            .ok()
            .flatten()
            .unwrap_or_else(|| "60".into())
    });
    let mut svc_mqtt_enabled = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_weather_mqtt_enabled", false)
    });
    let mut svc_mqtt_broker = use_signal(|| {
        state()
            .db
            .setting("svc_weather_mqtt_broker")
            .ok()
            .flatten()
            .unwrap_or_default()
    });
    let mut svc_mqtt_port = use_signal(|| {
        state()
            .db
            .setting("svc_weather_mqtt_port")
            .ok()
            .flatten()
            .unwrap_or_else(|| "1883".into())
    });
    let mut svc_mqtt_tls = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_weather_mqtt_tls", false)
    });
    let mut svc_mqtt_user = use_signal(|| {
        state()
            .db
            .setting("svc_weather_mqtt_username")
            .ok()
            .flatten()
            .unwrap_or_default()
    });
    let mut svc_mqtt_pass = use_signal(String::new);
    let mut svc_mqtt_topic = use_signal(|| {
        state()
            .db
            .setting("svc_weather_mqtt_topic")
            .ok()
            .flatten()
            .unwrap_or_default()
    });
    let mut svc_mqtt_mode = use_signal(|| {
        state()
            .db
            .setting("svc_weather_mqtt_mode")
            .ok()
            .flatten()
            .unwrap_or_else(|| "json_template".into())
    });
    let mut svc_mqtt_template = use_signal(|| {
        state()
            .db
            .setting("svc_weather_mqtt_template")
            .ok()
            .flatten()
            .unwrap_or_default()
    });
    let mut svc_mqtt_stale = use_signal(|| {
        state()
            .db
            .setting("svc_weather_station_stale_secs")
            .ok()
            .flatten()
            .or_else(|| {
                state()
                    .db
                    .setting("svc_weather_mqtt_stale_secs")
                    .ok()
                    .flatten()
            })
            .unwrap_or_else(|| "3600".into())
    });
    let mut svc_mqtt_label = use_signal(|| {
        state()
            .db
            .setting("svc_weather_mqtt_label")
            .ok()
            .flatten()
            .unwrap_or_default()
    });
    let mut svc_ecowitt_push = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_weather_ecowitt_push_enabled", false)
    });
    let mut svc_ecowitt_port = use_signal(|| {
        state()
            .db
            .setting("svc_weather_ecowitt_push_port")
            .ok()
            .flatten()
            .unwrap_or_else(|| "4199".into())
    });
    let mut svc_ecowitt_path = use_signal(|| {
        state()
            .db
            .setting("svc_weather_ecowitt_push_path")
            .ok()
            .flatten()
            .unwrap_or_else(|| "/data/report".into())
    });
    let mut svc_ecowitt_passkey = use_signal(|| {
        state()
            .db
            .setting("svc_weather_ecowitt_passkey")
            .ok()
            .flatten()
            .unwrap_or_default()
    });
    let mut svc_ecowitt_pull = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_weather_ecowitt_pull_enabled", false)
    });
    let mut svc_ecowitt_host = use_signal(|| {
        state()
            .db
            .setting("svc_weather_ecowitt_pull_host")
            .ok()
            .flatten()
            .unwrap_or_default()
    });
    let mut local_api_enabled = use_signal(|| {
        crate::services::setting_bool(&state().db, "local_api_enabled", false)
    });
    let mut local_api_bind = use_signal(|| {
        state()
            .db
            .setting("local_api_bind")
            .ok()
            .flatten()
            .unwrap_or_else(|| "127.0.0.1".into())
    });
    let mut local_api_port = use_signal(|| {
        state()
            .db
            .setting("local_api_port")
            .ok()
            .flatten()
            .unwrap_or_else(|| "8787".into())
    });
    let mut local_api_prefix = use_signal(|| {
        state()
            .db
            .setting("local_api_prefix")
            .ok()
            .flatten()
            .unwrap_or_else(|| "/api/v1".into())
    });
    let mut local_api_rate = use_signal(|| {
        state()
            .db
            .setting("local_api_rate_limit_per_min")
            .ok()
            .flatten()
            .unwrap_or_else(|| "120".into())
    });
    let mut local_api_token_reveal = use_signal(String::new);
    let mut ha_mqtt_enabled = use_signal(|| {
        crate::services::setting_bool(&state().db, "ha_mqtt_enabled", false)
    });
    let mut ha_mqtt_broker = use_signal(|| {
        state()
            .db
            .setting("ha_mqtt_broker")
            .ok()
            .flatten()
            .unwrap_or_default()
    });
    let mut ha_mqtt_port = use_signal(|| {
        state()
            .db
            .setting("ha_mqtt_port")
            .ok()
            .flatten()
            .unwrap_or_else(|| "1883".into())
    });
    let mut ha_mqtt_tls = use_signal(|| {
        crate::services::setting_bool(&state().db, "ha_mqtt_tls", false)
    });
    let mut ha_mqtt_user = use_signal(|| {
        state()
            .db
            .setting("ha_mqtt_username")
            .ok()
            .flatten()
            .unwrap_or_default()
    });
    let mut ha_mqtt_pass = use_signal(String::new);
    let mut ha_mqtt_discovery_prefix = use_signal(|| {
        state()
            .db
            .setting("ha_mqtt_discovery_prefix")
            .ok()
            .flatten()
            .unwrap_or_else(|| "homeassistant".into())
    });
    let mut ha_mqtt_node_id = use_signal(|| {
        state()
            .db
            .setting("ha_mqtt_node_id")
            .ok()
            .flatten()
            .unwrap_or_else(|| "metis_mesh_hq".into())
    });
    let mut ha_mqtt_state_prefix = use_signal(|| {
        state()
            .db
            .setting("ha_mqtt_state_prefix")
            .ok()
            .flatten()
            .unwrap_or_else(|| "metis_mesh_hq".into())
    });
    let mut ha_mqtt_interval = use_signal(|| {
        state()
            .db
            .setting("ha_mqtt_interval_secs")
            .ok()
            .flatten()
            .unwrap_or_else(|| "30".into())
    });
    let mut svc_ecowitt_interval = use_signal(|| {
        state()
            .db
            .setting("svc_weather_ecowitt_pull_interval_secs")
            .ok()
            .flatten()
            .unwrap_or_else(|| "30".into())
    });
    let mut svc_amber = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_amber_enabled", false)
    });
    let mut svc_amber_auto = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_amber_auto_relay", false)
    });
    let mut svc_emerg = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_emerg_enabled", false)
    });
    let mut svc_emerg_auto = use_signal(|| {
        crate::services::setting_bool(&state().db, "svc_emerg_auto_relay", false)
    });
    let mut svc_emerg_state = use_signal(|| {
        state()
            .db
            .setting("svc_emerg_state")
            .ok()
            .flatten()
            .unwrap_or_else(|| "CA".into())
    });
    let mut log_tick = use_signal(|| 0u32);
    let mut log_poll_started = use_signal(|| false);
    let mut presets_tick = use_signal(|| 0u32);
    let time_preview = format_time(Some(unix_now()));
    let linked = radio.state == "connected";
    let max_tx = radio.max_tx_power.unwrap_or(22);
    let _presets_refresh = presets_tick();
    let presets = community_presets();
    let preset_q = preset_query().to_lowercase();
    let filtered_presets: Vec<RadioPreset> = presets
        .iter()
        .filter(|preset| {
            preset_q.is_empty()
                || preset.name.to_lowercase().contains(&preset_q)
                || preset.summary().to_lowercase().contains(&preset_q)
        })
        .cloned()
        .collect();
    let current_freq = radio.radio_freq_mhz.unwrap_or(910.525);
    let current_bw = radio.radio_bw_khz.unwrap_or(62.5);
    let current_sf = radio.radio_sf.unwrap_or(7);
    let current_cr = radio.radio_cr.unwrap_or(5);
    let active_preset = preset_label_for(current_freq, current_bw, current_sf, current_cr);
    let path_hash_key = path_hash_var_key(&radio.custom_vars);
    let gps_key = gps_enabled_var_key(&radio.custom_vars);
    let gps_interval_key = gps_interval_var_key(&radio.custom_vars);
    let share_loc_key = share_loc_var_key(&radio.custom_vars);
    let path_hash_mode = radio.path_hash_mode.or_else(|| {
        path_hash_key
            .as_ref()
            .and_then(|key| radio.custom_vars.get(key))
            .and_then(|v| v.trim().parse::<u8>().ok())
    });
    let gps_on = gps_key
        .as_ref()
        .and_then(|key| radio.custom_vars.get(key))
        .and_then(|v| is_boolish_var(v))
        .unwrap_or(false);

    // Always refresh the diagnostics ring while Settings is open so connect/sync
    // lines appear live even if the operator is on another settings tab first.
    if !log_poll_started() {
        log_poll_started.set(true);
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(350)).await;
                log_tick.with_mut(|n| *n = n.wrapping_add(1));
            }
        });
    }

    use_effect(move || {
        if settings_tab() != SettingsTab::Device {
            return;
        }
        spawn(async move {
            let result = tokio::task::spawn_blocking(refresh_presets_cache_if_needed).await;
            match result {
                Ok(Ok(true)) => presets_tick.with_mut(|n| *n = n.wrapping_add(1)),
                Ok(Ok(false)) => {}
                Ok(Err(err)) => hq_log!(warn, "meshcore presets refresh: {err}"),
                Err(err) => hq_log!(warn, "meshcore presets refresh join: {err}"),
            }
        });
    });

    use_effect(move || {
        if settings_tab() != SettingsTab::Hq || hq_tab() != HqTab::Connect {
            return;
        }
        if usb_scanned() {
            return;
        }
        usb_scanned.set(true);
        state().radio.scan_usb();
    });

    // Readiness (catalog + per-file metadata) is too heavy for the 350ms poll —
    // refresh only when entering Flasher or after sync/tick bumps.
    use_effect(move || {
        if settings_tab() != SettingsTab::Flasher || flasher_manage_open() {
            return;
        }
        let _ = flasher_tick();
        spawn(async move {
            let report = tokio::task::spawn_blocking(crate::firmware::readiness_snapshot).await;
            if let Ok(report) = report {
                flasher_readiness.set(Some(report));
            }
        });
    });

    // Pause the global 350ms poll while Manage firmware is open — rebuilding
    // hundreds of rows + rescanning the asset index each tick freezes the UI.
    // Also slow the Flasher tab: full catalog/index rebuild every 350ms locks the UI.
    let flasher_manage_blocking = settings_tab() == SettingsTab::Flasher && flasher_manage_open();
    let on_flasher = settings_tab() == SettingsTab::Flasher;
    if !flasher_manage_blocking {
        // Always subscribe so Diagnostics / sync notes stay live; Flasher uses flasher_tick.
        let _log_refresh = log_tick();
        let _ = on_flasher;
    }
    let log_rows = if settings_tab() == SettingsTab::Diagnostics {
        crate::app_log::filtered(
            crate::app_log::LogFilter::from_value(&log_filter()),
            &log_query(),
        )
    } else {
        Vec::new()
    };
    let log_count = log_rows.len();
    use_effect(move || {
        let _ = log_tick();
        let _ = log_count;
        if settings_tab() != SettingsTab::Diagnostics || !log_autoscroll() {
            return;
        }
        spawn(async move {
            // After paint, pin the transcript to the newest lines.
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            let _ = document::eval(
                r#"const el = document.getElementById('hq-log-transcript');
                   if (el) { el.scrollTop = el.scrollHeight; }"#,
            );
        });
    });

    let query = device_query().to_lowercase();
    let devices: Vec<BleRadio> = radio
        .devices
        .iter()
        .filter(|device| {
            query.is_empty()
                || device.name.to_lowercase().contains(&query)
                || device.address.to_lowercase().contains(&query)
                || device.manufacturer.to_lowercase().contains(&query)
        })
        .cloned()
        .collect();
    let companion_count = radio.devices.iter().filter(|device| device.companion).count();
    let mqtt_wx_status = crate::services::mqtt_weather_status_line(&state().db);
    let ecowitt_push_status = crate::services::ecowitt_push_status_line(&state().db);
    let ecowitt_pull_status = crate::services::ecowitt_pull_status_line(&state().db);
    let local_api_status = crate::services::local_api_status_line(&state().db);
    let local_api_has_token = crate::services::local_api_token_present(&crate::data_dir());
    let ha_mqtt_status = crate::services::ha_mqtt_status_line(&state().db);
    let ha_mqtt_pass_saved = crate::services::ha_mqtt_password_present(&crate::data_dir());
    let mqtt_template_default =
        crate::services::default_json_template(crate::services::weather_unit_f(&state().db));
    let mqtt_pass_saved = crate::services::password_file_present(&crate::data_dir());
    let scanning = radio.state == "scanning" || radio.state == "pairing";
    let usb_scanning = radio.state == "scanning_usb";
    let busy = matches!(
        radio.state.as_str(),
        "connecting" | "reconnecting" | "pairing" | "scanning" | "scanning_usb" | "disconnecting"
    );
    let target_saved = state()
        .db
        .is_saved_radio(&radio_name())
        .unwrap_or(false);

    let preview = crate::db::Db::message_keep_cutoff(&message_keep()).and_then(|cutoff| {
        state()
            .db
            .purge_preview(cutoff)
            .ok()
            .map(|preview| (cutoff, preview))
    });
    let infra_preview =
        crate::db::Db::infra_status_keep_cutoff(&infra_status_keep()).and_then(|cutoff| {
            state()
                .db
                .purge_preview(cutoff)
                .ok()
                .map(|preview| (cutoff, preview.infra_status))
        });

    rsx! {
        div { class: "mx-auto flex max-w-4xl flex-col gap-4",
            PageHeading { icon: IconName::Settings, title: "Settings".to_string(),
                "Headquarters app preferences and MeshCore companion device configuration."
            }
            div { class: "settings-tabs",
                button {
                    class: if settings_tab() == SettingsTab::Hq { "settings-tab settings-tab-active" } else { "settings-tab" },
                    onclick: move |_| settings_tab.set(SettingsTab::Hq),
                    Icon { name: IconName::Settings, size: 14 }
                    "HQ"
                }
                button {
                    class: if settings_tab() == SettingsTab::Device { "settings-tab settings-tab-active" } else { "settings-tab" },
                    onclick: move |_| settings_tab.set(SettingsTab::Device),
                    Icon { name: IconName::Radio, size: 14 }
                    "Device"
                }
                button {
                    class: if settings_tab() == SettingsTab::Flasher { "settings-tab settings-tab-active" } else { "settings-tab" },
                    onclick: move |_| settings_tab.set(SettingsTab::Flasher),
                    Icon { name: IconName::Radio, size: 14 }
                    "Flasher"
                }
                button {
                    class: if settings_tab() == SettingsTab::Diagnostics { "settings-tab settings-tab-active" } else { "settings-tab" },
                    onclick: move |_| settings_tab.set(SettingsTab::Diagnostics),
                    Icon { name: IconName::Messages, size: 14 }
                    "Diagnostics"
                }
                button {
                    class: if settings_tab() == SettingsTab::About { "settings-tab settings-tab-active" } else { "settings-tab" },
                    onclick: move |_| settings_tab.set(SettingsTab::About),
                    Icon { name: IconName::Radio, size: 14 }
                    "About"
                }
            }
            if settings_tab() == SettingsTab::Hq {
            div { class: "settings-subtabs",
                button {
                    class: if hq_tab() == HqTab::Connect { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                    onclick: move |_| hq_tab.set(HqTab::Connect),
                    Icon { name: IconName::Bluetooth, size: 14 }
                    "Connect"
                }
                button {
                    class: if hq_tab() == HqTab::Preferences { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                    onclick: move |_| hq_tab.set(HqTab::Preferences),
                    Icon { name: IconName::Settings, size: 14 }
                    "Preferences"
                }
                button {
                    class: if hq_tab() == HqTab::Services { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                    onclick: move |_| hq_tab.set(HqTab::Services),
                    Icon { name: IconName::Signal, size: 14 }
                    "Services"
                }
                button {
                    class: if hq_tab() == HqTab::Data { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                    onclick: move |_| hq_tab.set(HqTab::Data),
                    Icon { name: IconName::Key, size: 14 }
                    "Data"
                }
            }
            div { class: "hq-settings-panel",
            if hq_tab() == HqTab::Connect {
            {
                let pool = state().radio.pool_snapshot();
                let usb_ports = radio.usb_ports.clone();
                rsx! {
            Card { title: "Radio pool", icon: Some(IconName::Radio),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Several companions can stay linked at once. Only the primary companion sends enrollment secrets, bot replies, and operator messages. Secondaries still ingest contacts and traffic."
                }
                if pool.is_empty() {
                    p { class: "text-sm text-muted-foreground",
                        "None yet. Connect USB or Network below, or scan Bluetooth and Pair once."
                    }
                } else {
                    div { class: "flex flex-col gap-2",
                        for slot in pool {
                            {
                                let slot_id = slot.id;
                                let slot_addr = slot.address.clone();
                                let slot_connected = slot.connected;
                                let slot_primary = slot.is_primary;
                                let transport_badge = transport_badge(&slot.transport);
                                rsx! {
                                    div {
                                        key: "{slot.id}",
                                        class: "flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border bg-background px-3 py-3",
                                        div { class: "min-w-0 flex-1",
                                            div { class: "mb-1 flex flex-wrap items-center gap-2",
                                                p { class: "truncate font-medium", "{slot.label}" }
                                                Badge {
                                                    tone: if slot_connected {
                                                        "enrolled".to_string()
                                                    } else {
                                                        "pending".to_string()
                                                    },
                                                    "{transport_badge}"
                                                }
                                                if slot_primary {
                                                    Badge { tone: "signal".to_string(), "Primary" }
                                                }
                                                if slot_connected {
                                                    Badge { tone: "ok".to_string(), "{slot.state}" }
                                                }
                                            }
                                            p { class: "font-mono text-xs text-muted-foreground", "{slot.address}" }
                                            if !slot.detail.is_empty() {
                                                p { class: "mt-1 text-xs text-muted-foreground", "{slot.detail}" }
                                            }
                                            if let Some(pct) = slot.battery_pct {
                                                p { class: "mt-1 text-xs text-muted-foreground", "Battery {pct}%" }
                                            }
                                        }
                                        div { class: "flex flex-wrap items-center gap-2",
                                            div { class: "w-36",
                                                SelectInput {
                                                    id: format!("radio-role-{slot_id}"),
                                                    value: slot.role.clone(),
                                                    onchange: move |event: FormEvent| {
                                                        state().radio.set_role(slot_id, event.value());
                                                    },
                                                    option { value: "companion", style: "background-color: #151a20; color: #f4f4f5;", "Companion" }
                                                    option { value: "room", style: "background-color: #151a20; color: #f4f4f5;", "Room server" }
                                                    option { value: "repeater", style: "background-color: #151a20; color: #f4f4f5;", "Repeater" }
                                                    option { value: "other", style: "background-color: #151a20; color: #f4f4f5;", "Other" }
                                                }
                                            }
                                            if slot_connected && slot.role == "companion" && !slot_primary {
                                                Button {
                                                    kind: ButtonKind::Ghost,
                                                    onclick: move |_| state().radio.set_primary(slot_id),
                                                    "Make primary"
                                                }
                                            }
                                            if slot_connected {
                                                Button {
                                                    kind: ButtonKind::Ghost,
                                                    onclick: move |_| state().radio.disconnect_radio(slot_id),
                                                    Icon { name: IconName::Unlink, size: 14 }
                                                    "Disconnect"
                                                }
                                            } else {
                                                Button {
                                                    disabled: busy,
                                                    onclick: move |_| {
                                                        radio_name.set(slot_addr.clone());
                                                        state().radio.reconnect(slot_addr.clone());
                                                    },
                                                    Icon {
                                                        name: transport_icon(&slot.transport),
                                                        size: 14
                                                    }
                                                    "Connect"
                                                }
                                            }
                                            Button {
                                                kind: ButtonKind::Ghost,
                                                onclick: move |_| state().radio.forget_radio(slot_id),
                                                Icon { name: IconName::Trash, size: 14 }
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
            Card { title: "Add companion".to_string(), icon: Some(IconName::Radio),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Link a MeshCore companion over Bluetooth, USB, or the LAN. Several can stay in the pool; only the primary sends enrollment and bot traffic."
                }
                div { class: "settings-subtabs mb-3",
                    button {
                        class: if connect_link_tab() == ConnectLinkTab::Bluetooth {
                            "settings-subtab settings-subtab-active"
                        } else {
                            "settings-subtab"
                        },
                        onclick: move |_| connect_link_tab.set(ConnectLinkTab::Bluetooth),
                        Icon { name: IconName::Bluetooth, size: 14 }
                        "Bluetooth"
                    }
                    button {
                        class: if connect_link_tab() == ConnectLinkTab::Usb {
                            "settings-subtab settings-subtab-active"
                        } else {
                            "settings-subtab"
                        },
                        onclick: move |_| {
                            connect_link_tab.set(ConnectLinkTab::Usb);
                            state().radio.scan_usb();
                        },
                        Icon { name: IconName::Usb, size: 14 }
                        "USB"
                    }
                    button {
                        class: if connect_link_tab() == ConnectLinkTab::Network {
                            "settings-subtab settings-subtab-active"
                        } else {
                            "settings-subtab"
                        },
                        onclick: move |_| connect_link_tab.set(ConnectLinkTab::Network),
                        Icon { name: IconName::Signal, size: 14 }
                        "Network"
                    }
                }
                if connect_link_tab() == ConnectLinkTab::Bluetooth {
                    p { class: "mb-3 text-sm text-muted-foreground",
                        "Scan for nearby devices. Connect for a saved radio; Pair for a new one (six-digit PIN). Disconnect the phone app first."
                    }
                    div { class: "mb-3 flex flex-wrap gap-2",
                        div { class: "min-w-0 flex-1 basis-48",
                            TextInput {
                                value: radio_name(),
                                placeholder: "Name or AA:BB:CC:DD:EE:FF".to_string(),
                                oninput: move |event: FormEvent| radio_name.set(event.value()),
                            }
                        }
                        if target_saved {
                            Button {
                                disabled: busy || radio_name().trim().is_empty(),
                                onclick: move |_| state().radio.reconnect(radio_name()),
                                Icon { name: IconName::Bluetooth, size: 14 }
                                "Connect"
                            }
                        } else {
                            Button {
                                onclick: move |_| on_request_pin.call(radio_name()),
                                Icon { name: IconName::Key, size: 14 }
                                "Pair"
                            }
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            disabled: scanning,
                            onclick: move |_| state().radio.scan(),
                            if scanning {
                                Throbber { size: 14 }
                            } else {
                                Icon { name: IconName::Scan, size: 14 }
                            }
                            if scanning { "Scanning…" } else { "Scan" }
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| state().radio.disconnect(),
                            Icon { name: IconName::Unlink, size: 14 }
                            "Disconnect primary"
                        }
                    }
                    if !radio.devices.is_empty() {
                        div { class: "mb-3 flex items-center justify-between gap-3",
                            p { class: "text-xs text-muted-foreground",
                                "{radio.devices.len()} devices, {companion_count} MeshCore"
                            }
                            div { class: "w-56",
                                TextInput {
                                    value: device_query(),
                                    placeholder: "Filter name or address".to_string(),
                                    oninput: move |event: FormEvent| device_query.set(event.value()),
                                }
                            }
                        }
                        div { class: "flex max-h-[32rem] flex-col gap-2 overflow-y-auto pr-1",
                            if devices.is_empty() {
                                p { class: "text-sm text-muted-foreground", "No devices match that filter." }
                            }
                            for device in devices {
                                DeviceCard {
                                    key: "{device.address}",
                                    device: device.clone(),
                                    selected: radio_name() == device.address || radio_name() == device.name,
                                    connected: radio.state == "connected"
                                        && (radio.connected_name == device.address
                                            || radio.connected_name == device.name),
                                    paired: state().db.is_saved_radio(&device.address).unwrap_or(false)
                                        || state().db.is_saved_radio(&device.name).unwrap_or(false),
                                    on_use: {
                                        let address = device.address.clone();
                                        let paired = state().db.is_saved_radio(&device.address).unwrap_or(false)
                                            || state().db.is_saved_radio(&device.name).unwrap_or(false);
                                        move |_| {
                                            radio_name.set(address.clone());
                                            if paired {
                                                state().radio.reconnect(address.clone());
                                            } else {
                                                on_request_pin.call(address.clone());
                                            }
                                        }
                                    },
                                }
                            }
                        }
                    }
                } else if connect_link_tab() == ConnectLinkTab::Usb {
                    p { class: "mb-3 text-sm text-muted-foreground",
                        "USB companion firmware (CDC). BLE-only builds usually leave this as console. Linux prefers by-id paths; Windows COM; macOS cu.usbmodem. No PIN."
                    }
                    div { class: "mb-3 flex flex-wrap gap-2",
                        Button {
                            kind: ButtonKind::Ghost,
                            disabled: usb_scanning,
                            onclick: move |_| state().radio.scan_usb(),
                            if usb_scanning {
                                Throbber { size: 14 }
                            } else {
                                Icon { name: IconName::Scan, size: 14 }
                            }
                            if usb_scanning { "Refreshing…" } else { "Refresh ports" }
                        }
                    }
                    if usb_ports.is_empty() {
                        p { class: "text-sm text-muted-foreground",
                            "No USB serial ports found. Refresh after plugging in a companion."
                        }
                    } else {
                        div { class: "flex flex-col gap-2",
                            for port in usb_ports {
                                div {
                                    key: "{port.path}",
                                    class: "flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border bg-background px-3 py-3",
                                    div { class: "min-w-0",
                                        p { class: "truncate font-medium", "{port.label}" }
                                        p { class: "font-mono text-xs text-muted-foreground", "{port.path}" }
                                        if port.system_path != port.path {
                                            p { class: "font-mono text-[0.65rem] text-muted-foreground", "also {port.system_path}" }
                                        }
                                    }
                                    Button {
                                        disabled: busy,
                                        onclick: {
                                            let path = port.path.clone();
                                            move |_| state().radio.connect_usb(path.clone())
                                        },
                                        Icon { name: IconName::Usb, size: 14 }
                                        "Connect USB"
                                    }
                                }
                            }
                        }
                    }
                } else {
                    p { class: "mb-3 text-sm text-muted-foreground",
                        "Wi-Fi or Ethernet companion on the LAN over TCP (default port 5000). Same MeshCore protocol as BLE/USB; no PIN."
                    }
                    div { class: "grid gap-3 sm:grid-cols-[minmax(0,1fr)_7rem_auto] sm:items-end",
                        div { class: "min-w-0",
                            label {
                                class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                "Host or IP"
                            }
                            TextInput {
                                value: tcp_host(),
                                placeholder: "192.168.1.50 or companion.local".to_string(),
                                oninput: move |event: FormEvent| tcp_host.set(event.value()),
                            }
                        }
                        div {
                            label {
                                class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                "Port"
                            }
                            TextInput {
                                value: tcp_port(),
                                placeholder: "5000".to_string(),
                                oninput: move |event: FormEvent| tcp_port.set(event.value()),
                            }
                        }
                        Button {
                            disabled: busy || tcp_host().trim().is_empty(),
                            onclick: move |_| {
                                let host = tcp_host().trim().to_string();
                                let port = tcp_port().trim().to_string();
                                let raw = if port.is_empty() || port == "5000" {
                                    host
                                } else if host.contains(':') && !host.starts_with('[') {
                                    format!("[{host}]:{port}")
                                } else {
                                    format!("{host}:{port}")
                                };
                                state().radio.connect_tcp(raw);
                            },
                            Icon { name: IconName::Signal, size: 14 }
                            "Connect TCP"
                        }
                    }
                }
            }
                } // end connect rsx
            } // end connect block
            } // end HqTab::Connect

            if hq_tab() == HqTab::Preferences {
            Card { title: "Date & time", icon: Some(IconName::Clock),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Timestamps use this computer’s local timezone. Preview: {time_preview}"
                }
                div { class: "grid gap-3 sm:grid-cols-2",
                    div {
                        label {
                            class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground",
                            r#for: "time-format",
                            "Clock"
                        }
                        SelectInput {
                            id: "time-format".to_string(),
                            value: time_format(),
                            onchange: move |event: FormEvent| {
                                let value = event.value();
                                let _ = state().db.set_setting("time_format", &value);
                                time_format.set(value);
                            },
                            option {
                                value: "12h",
                                style: "background-color: #151a20; color: #f4f4f5;",
                                "12-hour (AM/PM)"
                            }
                            option {
                                value: "24h",
                                style: "background-color: #151a20; color: #f4f4f5;",
                                "24-hour"
                            }
                        }
                    }
                    div {
                        label {
                            class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground",
                            r#for: "date-format",
                            "Date"
                        }
                        SelectInput {
                            id: "date-format".to_string(),
                            value: date_format(),
                            onchange: move |event: FormEvent| {
                                let value = event.value();
                                let _ = state().db.set_setting("date_format", &value);
                                date_format.set(value);
                            },
                            option {
                                value: "md_short",
                                style: "background-color: #151a20; color: #f4f4f5;",
                                "Oct 31"
                            }
                            option {
                                value: "mdy_long",
                                style: "background-color: #151a20; color: #f4f4f5;",
                                "Oct 31, 2026"
                            }
                            option {
                                value: "mdy",
                                style: "background-color: #151a20; color: #f4f4f5;",
                                "10/31/2026"
                            }
                            option {
                                value: "dmy",
                                style: "background-color: #151a20; color: #f4f4f5;",
                                "31/10/2026"
                            }
                            option {
                                value: "ymd",
                                style: "background-color: #151a20; color: #f4f4f5;",
                                "2026-10-31"
                            }
                        }
                    }
                }
            }
            } // end HqTab::Preferences (date/time)

            if hq_tab() == HqTab::Services {
            Card { title: "Info services", icon: Some(IconName::Radio),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Commands run on the private {channel_info_name()} channel (enrolled secret only). Alerts are labeled with source and time — not an official agency feed."
                }
                div { class: "settings-subtabs",
                    button {
                        class: if services_tab() == ServicesTab::Site { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                        onclick: move |_| services_tab.set(ServicesTab::Site),
                        "Site"
                    }
                    button {
                        class: if services_tab() == ServicesTab::Weather { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                        onclick: move |_| services_tab.set(ServicesTab::Weather),
                        "Weather"
                    }
                    button {
                        class: if services_tab() == ServicesTab::Ecowitt { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                        onclick: move |_| services_tab.set(ServicesTab::Ecowitt),
                        "Ecowitt"
                    }
                        button {
                            class: if services_tab() == ServicesTab::Mqtt { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                            onclick: move |_| services_tab.set(ServicesTab::Mqtt),
                            "MQTT"
                        }
                        button {
                            class: if services_tab() == ServicesTab::LocalApi { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                            onclick: move |_| services_tab.set(ServicesTab::LocalApi),
                            "Local API"
                        }
                        button {
                            class: if services_tab() == ServicesTab::Alerts { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                            onclick: move |_| services_tab.set(ServicesTab::Alerts),
                            "Alerts"
                        }
                    button {
                        class: if services_tab() == ServicesTab::Quake { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                        onclick: move |_| services_tab.set(ServicesTab::Quake),
                        "Quake"
                    }
                }

                if services_tab() == ServicesTab::Site {
                    div { class: "services-panel",
                        p { class: "text-sm text-muted-foreground",
                            "Site pin anchors bare #weather, NWS polling, and the USGS box when a node has no GPS."
                        }
                        div { class: "grid gap-3 sm:grid-cols-2",
                            div {
                                label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "HQ site latitude" }
                                TextInput {
                                    value: svc_site_lat(),
                                    placeholder: radio.hq_lat.map(|v| format!("{v:.6}")).unwrap_or_else(|| "34.050000".into()),
                                    oninput: move |event: FormEvent| svc_site_lat.set(event.value()),
                                }
                            }
                            div {
                                label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "HQ site longitude" }
                                TextInput {
                                    value: svc_site_lon(),
                                    placeholder: radio.hq_lon.map(|v| format!("{v:.6}")).unwrap_or_else(|| "-118.250000".into()),
                                    oninput: move |event: FormEvent| svc_site_lon.set(event.value()),
                                }
                            }
                        }
                        div { class: "flex flex-wrap gap-2",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    let lat = svc_site_lat().trim().to_string();
                                    let lon = svc_site_lon().trim().to_string();
                                    if lat.parse::<f64>().is_ok() && lon.parse::<f64>().is_ok() {
                                        let _ = state().db.set_setting("hq_site_lat", &lat);
                                        let _ = state().db.set_setting("hq_site_lon", &lon);
                                        status_note("HQ site pin saved for weather and NWS.");
                                    } else {
                                        status_note("Enter valid decimal latitude and longitude.");
                                    }
                                },
                                Icon { name: IconName::Check, size: 14 }
                                "Save site pin"
                            }
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    if let (Some(lat), Some(lon)) = (radio.hq_lat, radio.hq_lon) {
                                        let lat_s = format!("{lat:.6}");
                                        let lon_s = format!("{lon:.6}");
                                        let _ = state().db.set_setting("hq_site_lat", &lat_s);
                                        let _ = state().db.set_setting("hq_site_lon", &lon_s);
                                        svc_site_lat.set(lat_s);
                                        svc_site_lon.set(lon_s);
                                        status_note("Copied companion advert coords into the site pin.");
                                    } else {
                                        status_note("Companion has no advert coordinates yet.");
                                    }
                                },
                                "Use companion coords"
                            }
                        }
                    }
                }

                if services_tab() == ServicesTab::Weather {
                    div { class: "services-panel",
                        ToggleSwitch {
                            checked: svc_weather(),
                            label: format!("Weather on {0}", channel_info_name()),
                            hint: Some("#weather [zip] and #weather+ (multi-frame). Place lookups use Open-Meteo.".to_string()),
                            onchange: move |next| {
                                svc_weather.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_weather_enabled", next);
                            }
                        }
                        div { class: "w-40",
                            label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "Units" }
                            SelectInput {
                                id: "svc-weather-unit".to_string(),
                                value: svc_weather_unit(),
                                onchange: move |event: FormEvent| {
                                    let value = event.value();
                                    let _ = state().db.set_setting("svc_weather_unit", &value);
                                    svc_weather_unit.set(value);
                                },
                                option { value: "F", style: "background-color: #151a20; color: #f4f4f5;", "Fahrenheit" }
                                option { value: "C", style: "background-color: #151a20; color: #f4f4f5;", "Celsius" }
                            }
                        }
                        ToggleSwitch {
                            checked: svc_weather_hourly(),
                            disabled: !svc_weather(),
                            label: "Auto-post weather for ZIP list".to_string(),
                            hint: Some("Posts one ZIP per minute when due. Silent otherwise.".to_string()),
                            onchange: move |next| {
                                svc_weather_hourly.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_weather_hourly", next);
                            }
                        }
                        div { class: "flex flex-col gap-2",
                            label { class: "block text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                "Area ZIP codes (comma or space separated)"
                            }
                            TextInput {
                                value: svc_weather_zips(),
                                placeholder: "92345 92344 91709",
                                oninput: move |event: FormEvent| {
                                    let value = event.value();
                                    svc_weather_zips.set(value.clone());
                                    let _ = state().db.set_setting("svc_weather_zips", value.trim());
                                },
                            }
                            div { class: "flex items-center gap-2",
                                label { class: "text-xs text-muted-foreground shrink-0", "Every" }
                                div { class: "w-24",
                                    TextInput {
                                        value: svc_weather_interval(),
                                        placeholder: "60",
                                        oninput: move |event: FormEvent| {
                                            let value = event.value();
                                            svc_weather_interval.set(value.clone());
                                            if let Ok(mins) = value.parse::<i64>() {
                                                if (15..=24 * 60).contains(&mins) {
                                                    let _ = state().db.set_setting(
                                                        "svc_weather_interval_mins",
                                                        &mins.to_string(),
                                                    );
                                                }
                                            }
                                        },
                                    }
                                }
                                span { class: "text-xs text-muted-foreground", "minutes" }
                            }
                        }
                    }
                }

                if services_tab() == ServicesTab::Ecowitt {
                    div { class: "services-panel",
                        p { class: "text-sm text-muted-foreground",
                            "Local-only Ecowitt / Fine Offset gateways (GW1200, WittBoy/GW2001, GW300x). No cloud Ambient support. Push = Customized upload to this HQ. Pull = poll the gateway’s get_livedata_info. See the Weather page for the live dashboard."
                        }
                        p { class: "text-xs text-muted-foreground", "{ecowitt_push_status}" }
                        p { class: "text-xs text-muted-foreground", "{ecowitt_pull_status}" }
                        ToggleSwitch {
                            checked: svc_ecowitt_push(),
                            label: "Ecowitt custom-server push".to_string(),
                            hint: Some("Gateway Customized → protocol Ecowitt → this machine’s LAN IP, path, and port (HTTP only).".to_string()),
                            onchange: move |next| {
                                svc_ecowitt_push.set(next);
                                crate::services::set_setting_bool(
                                    &state().db,
                                    "svc_weather_ecowitt_push_enabled",
                                    next,
                                );
                                crate::services::bump_ecowitt_config();
                            }
                        }
                        div { class: "grid gap-2 sm:grid-cols-2",
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Listen port" }
                                TextInput {
                                    value: svc_ecowitt_port(),
                                    placeholder: "4199".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        svc_ecowitt_port.set(value.clone());
                                        if value.trim().parse::<u16>().is_ok() {
                                            let _ = state().db.set_setting(
                                                "svc_weather_ecowitt_push_port",
                                                value.trim(),
                                            );
                                            crate::services::bump_ecowitt_config();
                                        }
                                    },
                                }
                            }
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Path" }
                                TextInput {
                                    value: svc_ecowitt_path(),
                                    placeholder: "/data/report".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        svc_ecowitt_path.set(value.clone());
                                        let _ = state().db.set_setting(
                                            "svc_weather_ecowitt_push_path",
                                            value.trim(),
                                        );
                                        crate::services::bump_ecowitt_config();
                                    },
                                }
                            }
                        }
                        div { class: "flex flex-col gap-1",
                            label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "PASSKEY (optional)" }
                            TextInput {
                                value: svc_ecowitt_passkey(),
                                placeholder: "Leave blank to accept any".to_string(),
                                kind: "password".to_string(),
                                oninput: move |event: FormEvent| {
                                    let value = event.value();
                                    svc_ecowitt_passkey.set(value.clone());
                                    let _ = state().db.set_setting(
                                        "svc_weather_ecowitt_passkey",
                                        value.trim(),
                                    );
                                    crate::services::bump_ecowitt_config();
                                },
                            }
                        }
                        ToggleSwitch {
                            checked: svc_ecowitt_pull(),
                            label: "Poll gateway livedata API".to_string(),
                            hint: Some("GET http://gateway/get_livedata_info — IoT gateways GW1200 / GW2000 / GW3000.".to_string()),
                            onchange: move |next| {
                                svc_ecowitt_pull.set(next);
                                crate::services::set_setting_bool(
                                    &state().db,
                                    "svc_weather_ecowitt_pull_enabled",
                                    next,
                                );
                                crate::services::bump_ecowitt_config();
                            }
                        }
                        div { class: "grid gap-2 sm:grid-cols-2",
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Gateway host / IP" }
                                TextInput {
                                    value: svc_ecowitt_host(),
                                    placeholder: "192.168.1.50".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        svc_ecowitt_host.set(value.clone());
                                        let _ = state().db.set_setting(
                                            "svc_weather_ecowitt_pull_host",
                                            value.trim(),
                                        );
                                        crate::services::bump_ecowitt_config();
                                    },
                                }
                            }
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Interval (seconds)" }
                                TextInput {
                                    value: svc_ecowitt_interval(),
                                    placeholder: "30".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        svc_ecowitt_interval.set(value.clone());
                                        if let Ok(secs) = value.parse::<i64>() {
                                            if (5..=300).contains(&secs) {
                                                let _ = state().db.set_setting(
                                                    "svc_weather_ecowitt_pull_interval_secs",
                                                    &secs.to_string(),
                                                );
                                                crate::services::bump_ecowitt_config();
                                            }
                                        }
                                    },
                                }
                            }
                        }
                        div { class: "flex flex-col gap-1",
                            label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Station label (shared)" }
                            TextInput {
                                value: svc_mqtt_label(),
                                placeholder: "Yard".to_string(),
                                oninput: move |event: FormEvent| {
                                    let value = event.value();
                                    svc_mqtt_label.set(value.clone());
                                    let _ = state().db.set_setting(
                                        "svc_weather_mqtt_label",
                                        value.trim(),
                                    );
                                    crate::services::bump_mqtt_weather_config();
                                    crate::services::bump_ecowitt_config();
                                },
                            }
                        }
                        div { class: "flex items-center gap-2",
                            label { class: "text-xs text-muted-foreground shrink-0", "Stale after" }
                            div { class: "w-28",
                                TextInput {
                                    value: svc_mqtt_stale(),
                                    placeholder: "3600".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        svc_mqtt_stale.set(value.clone());
                                        if let Ok(secs) = value.parse::<i64>() {
                                            if (60..=7 * 24 * 3600).contains(&secs) {
                                                let _ = state().db.set_setting(
                                                    "svc_weather_station_stale_secs",
                                                    &secs.to_string(),
                                                );
                                                let _ = state().db.set_setting(
                                                    "svc_weather_mqtt_stale_secs",
                                                    &secs.to_string(),
                                                );
                                                crate::services::bump_mqtt_weather_config();
                                                crate::services::bump_ecowitt_config();
                                            }
                                        }
                                    },
                                }
                            }
                            span { class: "text-xs text-muted-foreground", "seconds (shared with MQTT)" }
                        }
                    }
                }

                if services_tab() == ServicesTab::Mqtt {
                    div { class: "services-panel",
                        p { class: "text-sm text-muted-foreground",
                            "Optional LAN MQTT bridge (Home Assistant / WeeWX → Mosquitto). Prefer the Ecowitt tab for direct gateway support. Bare #weather uses the freshest local station reading."
                        }
                        ToggleSwitch {
                            checked: svc_mqtt_enabled(),
                            disabled: !svc_weather(),
                            label: "Subscribe to local MQTT weather".to_string(),
                            hint: Some(if svc_weather() {
                                mqtt_wx_status.clone()
                            } else {
                                "Turn on Weather first.".to_string()
                            }),
                            onchange: move |next| {
                                svc_mqtt_enabled.set(next);
                                crate::services::set_setting_bool(
                                    &state().db,
                                    "svc_weather_mqtt_enabled",
                                    next,
                                );
                                crate::services::bump_mqtt_weather_config();
                            }
                        }
                        div { class: "grid gap-2 sm:grid-cols-2",
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Broker host" }
                                TextInput {
                                    value: svc_mqtt_broker(),
                                    placeholder: "127.0.0.1".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        svc_mqtt_broker.set(value.clone());
                                        let _ = state().db.set_setting(
                                            "svc_weather_mqtt_broker",
                                            value.trim(),
                                        );
                                        crate::services::bump_mqtt_weather_config();
                                    },
                                }
                            }
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Port" }
                                TextInput {
                                    value: svc_mqtt_port(),
                                    placeholder: "1883".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        svc_mqtt_port.set(value.clone());
                                        if value.trim().parse::<u16>().is_ok() {
                                            let _ = state().db.set_setting(
                                                "svc_weather_mqtt_port",
                                                value.trim(),
                                            );
                                            crate::services::bump_mqtt_weather_config();
                                        }
                                    },
                                }
                            }
                        }
                        ToggleSwitch {
                            checked: svc_mqtt_tls(),
                            disabled: !svc_mqtt_enabled(),
                            label: "TLS".to_string(),
                            onchange: move |next| {
                                svc_mqtt_tls.set(next);
                                crate::services::set_setting_bool(
                                    &state().db,
                                    "svc_weather_mqtt_tls",
                                    next,
                                );
                                crate::services::bump_mqtt_weather_config();
                            }
                        }
                        div { class: "grid gap-2 sm:grid-cols-2",
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Username (optional)" }
                                TextInput {
                                    value: svc_mqtt_user(),
                                    placeholder: "".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        svc_mqtt_user.set(value.clone());
                                        let _ = state().db.set_setting(
                                            "svc_weather_mqtt_username",
                                            value.trim(),
                                        );
                                        crate::services::bump_mqtt_weather_config();
                                    },
                                }
                            }
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                    if mqtt_pass_saved {
                                        "Password (saved in HQ data folder · leave blank to keep)"
                                    } else {
                                        "Password (saved as mqtt-weather.pass in HQ data folder · 0600)"
                                    }
                                }
                                TextInput {
                                    value: svc_mqtt_pass(),
                                    placeholder: "Broker password".to_string(),
                                    kind: "password".to_string(),
                                    oninput: move |event: FormEvent| svc_mqtt_pass.set(event.value()),
                                }
                                div { class: "flex gap-2",
                                    Button {
                                        kind: ButtonKind::Ghost,
                                        onclick: move |_| {
                                            let pass = svc_mqtt_pass();
                                            match crate::services::save_mqtt_weather_password(
                                                &crate::data_dir(),
                                                &pass,
                                            ) {
                                                Ok(()) => {
                                                    svc_mqtt_pass.set(String::new());
                                                    status_note(if pass.trim().is_empty() {
                                                        "MQTT weather password cleared."
                                                    } else {
                                                        "MQTT weather password saved."
                                                    });
                                                }
                                                Err(err) => status_note(format!("Password save failed: {err}")),
                                            }
                                        },
                                        "Save password"
                                    }
                                    Button {
                                        kind: ButtonKind::Ghost,
                                        onclick: move |_| {
                                            crate::services::clear_mqtt_weather_password(&crate::data_dir());
                                            svc_mqtt_pass.set(String::new());
                                            status_note("MQTT weather password cleared.");
                                        },
                                        "Clear"
                                    }
                                }
                            }
                        }
                        div { class: "flex flex-col gap-1",
                            label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Topic" }
                            TextInput {
                                value: svc_mqtt_topic(),
                                placeholder: "homeassistant/sensor/gw3002/state".to_string(),
                                oninput: move |event: FormEvent| {
                                    let value = event.value();
                                    svc_mqtt_topic.set(value.clone());
                                    let _ = state().db.set_setting(
                                        "svc_weather_mqtt_topic",
                                        value.trim(),
                                    );
                                    crate::services::bump_mqtt_weather_config();
                                },
                            }
                        }
                        div { class: "grid gap-2 sm:grid-cols-2",
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Payload mode" }
                                SelectInput {
                                    id: "svc-mqtt-mode".to_string(),
                                    value: svc_mqtt_mode(),
                                    onchange: move |event: FormEvent| {
                                        let value = event.value();
                                        let _ = state().db.set_setting("svc_weather_mqtt_mode", &value);
                                        svc_mqtt_mode.set(value);
                                        crate::services::bump_mqtt_weather_config();
                                    },
                                    option { value: "json_template", style: "background-color: #151a20; color: #f4f4f5;", "JSON template" }
                                    option { value: "passthrough", style: "background-color: #151a20; color: #f4f4f5;", "Passthrough text" }
                                }
                            }
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Station label" }
                                TextInput {
                                    value: svc_mqtt_label(),
                                    placeholder: "Yard".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        svc_mqtt_label.set(value.clone());
                                        let _ = state().db.set_setting(
                                            "svc_weather_mqtt_label",
                                            value.trim(),
                                        );
                                        crate::services::bump_mqtt_weather_config();
                                    },
                                }
                            }
                        }
                        div { class: "flex flex-col gap-1",
                            label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                "JSON template (blank = {mqtt_template_default})"
                            }
                            TextInput {
                                value: svc_mqtt_template(),
                                placeholder: mqtt_template_default.clone(),
                                oninput: move |event: FormEvent| {
                                    let value = event.value();
                                    svc_mqtt_template.set(value.clone());
                                    let _ = state().db.set_setting(
                                        "svc_weather_mqtt_template",
                                        value.trim(),
                                    );
                                    crate::services::bump_mqtt_weather_config();
                                },
                            }
                        }
                        div { class: "flex items-center gap-2",
                            label { class: "text-xs text-muted-foreground shrink-0", "Stale after" }
                            div { class: "w-28",
                                TextInput {
                                    value: svc_mqtt_stale(),
                                    placeholder: "3600".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        svc_mqtt_stale.set(value.clone());
                                        if let Ok(secs) = value.parse::<i64>() {
                                            if (60..=7 * 24 * 3600).contains(&secs) {
                                                let _ = state().db.set_setting(
                                                    "svc_weather_station_stale_secs",
                                                    &secs.to_string(),
                                                );
                                                let _ = state().db.set_setting(
                                                    "svc_weather_mqtt_stale_secs",
                                                    &secs.to_string(),
                                                );
                                                crate::services::bump_mqtt_weather_config();
                                            }
                                        }
                                    },
                                }
                            }
                            span { class: "text-xs text-muted-foreground", "seconds (shared; then bare #weather falls back to Open-Meteo)" }
                        }
                    }
                }

                if services_tab() == ServicesTab::LocalApi {
                    div { class: "services-panel",
                        p { class: "text-sm text-muted-foreground",
                            "Opt-in read-only JSON API for local apps and Home Assistant. Default bind is loopback. Bearer token is required on every request (including /health). Token is stored as local-api.token under the HQ data folder (0600) — never in the database. See Help → Local API for curl and Home Assistant examples."
                        }
                        p { class: "text-xs text-muted-foreground", "{local_api_status}" }
                        p { class: "text-xs text-muted-foreground",
                            strong { "Never exposed: " }
                            "channel secrets, keyring material, Compose/send, enroll/approve/deny, radio CLI, full message bodies, or GPS trails."
                        }
                        ToggleSwitch {
                            checked: local_api_enabled(),
                            label: "Enable local JSON API".to_string(),
                            hint: Some("GET /api/v1/health, /status, /nodes, /infra, /channels, /stats".to_string()),
                            onchange: move |next| {
                                let prev = crate::services::setting_bool(
                                    &state().db,
                                    "local_api_enabled",
                                    false,
                                );
                                if next {
                                    match crate::services::ensure_local_api_token(&crate::data_dir()) {
                                        Ok(token) => {
                                            local_api_token_reveal.set(token);
                                        }
                                        Err(err) => {
                                            crate::toast::push(
                                                crate::toast::ToastKind::Error,
                                                format!("Could not create API token: {err}"),
                                            );
                                            return;
                                        }
                                    }
                                }
                                local_api_enabled.set(next);
                                crate::services::set_setting_bool(
                                    &state().db,
                                    "local_api_enabled",
                                    next,
                                );
                                if prev != next {
                                    let _ = state().db.audit(
                                        if next { "local_api_enable" } else { "local_api_disable" },
                                        "",
                                        "",
                                    );
                                }
                                crate::services::bump_local_api_config();
                            }
                        }
                        if !crate::services::is_loopback_bind(&local_api_bind()) {
                            p { class: "text-xs text-amber-200",
                                "Bind is not loopback — anything on this LAN that knows the bearer token can read HQ status. Prefer 127.0.0.1 unless Home Assistant runs on another machine."
                            }
                        }
                        div { class: "grid gap-2 sm:grid-cols-2 lg:grid-cols-4",
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Bind" }
                                TextInput {
                                    value: local_api_bind(),
                                    placeholder: "127.0.0.1".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        local_api_bind.set(value.clone());
                                        let trimmed = value.trim();
                                        if trimmed.is_empty() {
                                            return;
                                        }
                                        let prev = state()
                                            .db
                                            .setting("local_api_bind")
                                            .ok()
                                            .flatten()
                                            .unwrap_or_default();
                                        if prev == trimmed {
                                            return;
                                        }
                                        let _ = state().db.set_setting("local_api_bind", trimmed);
                                        let _ = state().db.audit("local_api_bind", "", trimmed);
                                        crate::services::bump_local_api_config();
                                    },
                                }
                            }
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Port" }
                                TextInput {
                                    value: local_api_port(),
                                    placeholder: "8787".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        local_api_port.set(value.clone());
                                        let trimmed = value.trim();
                                        if trimmed.parse::<u16>().is_err() {
                                            return;
                                        }
                                        let prev = state()
                                            .db
                                            .setting("local_api_port")
                                            .ok()
                                            .flatten()
                                            .unwrap_or_default();
                                        if prev == trimmed {
                                            return;
                                        }
                                        let _ = state().db.set_setting("local_api_port", trimmed);
                                        let _ = state().db.audit("local_api_port", "", trimmed);
                                        crate::services::bump_local_api_config();
                                    },
                                }
                            }
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Path prefix" }
                                TextInput {
                                    value: local_api_prefix(),
                                    placeholder: "/api/v1".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        local_api_prefix.set(value.clone());
                                        let normalized =
                                            crate::services::normalize_local_api_prefix(value.trim());
                                        let prev = state()
                                            .db
                                            .setting("local_api_prefix")
                                            .ok()
                                            .flatten()
                                            .unwrap_or_default();
                                        if prev == normalized {
                                            return;
                                        }
                                        let _ = state().db.set_setting(
                                            "local_api_prefix",
                                            &normalized,
                                        );
                                        let _ = state().db.audit(
                                            "local_api_prefix",
                                            "",
                                            &normalized,
                                        );
                                        crate::services::bump_local_api_config();
                                    },
                                }
                            }
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Max requests / minute" }
                                TextInput {
                                    value: local_api_rate(),
                                    placeholder: "120".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        local_api_rate.set(value.clone());
                                        let Ok(parsed) = value.trim().parse::<u32>() else {
                                            return;
                                        };
                                        let clamped =
                                            crate::services::clamp_local_api_rate_limit(parsed);
                                        let stored = clamped.to_string();
                                        let prev = state()
                                            .db
                                            .setting("local_api_rate_limit_per_min")
                                            .ok()
                                            .flatten()
                                            .unwrap_or_default();
                                        if prev == stored {
                                            return;
                                        }
                                        let _ = state().db.set_setting(
                                            "local_api_rate_limit_per_min",
                                            &stored,
                                        );
                                        let _ = state().db.audit(
                                            "local_api_rate_limit",
                                            "",
                                            &stored,
                                        );
                                        crate::services::bump_local_api_config();
                                    },
                                }
                            }
                        }
                        div { class: "flex flex-col gap-2",
                            label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Bearer token" }
                            if !local_api_token_reveal().is_empty() {
                                TextInput {
                                    value: local_api_token_reveal(),
                                    placeholder: "".to_string(),
                                    oninput: move |_| {},
                                }
                                p { class: "text-xs text-muted-foreground",
                                    "Token shown after generate/rotate/enable. Copy it now — it is not stored in Settings history."
                                }
                            } else if local_api_has_token {
                                p { class: "text-xs text-muted-foreground",
                                    "A token file is present. Rotate to reveal a new value, or Copy if you just generated one this session."
                                }
                            } else {
                                p { class: "text-xs text-muted-foreground",
                                    "No token yet. Enabling the API or Generate will create one."
                                }
                            }
                            div { class: "flex flex-wrap gap-2",
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| {
                                        let token = crate::services::generate_local_api_token();
                                        match crate::services::save_local_api_token(
                                            &crate::data_dir(),
                                            &token,
                                        ) {
                                            Ok(()) => {
                                                local_api_token_reveal.set(token);
                                                let _ = state().db.audit(
                                                    "local_api_token_rotate",
                                                    "",
                                                    "",
                                                );
                                                crate::toast::push(
                                                    crate::toast::ToastKind::Success,
                                                    "API token saved.",
                                                );
                                            }
                                            Err(err) => {
                                                crate::toast::push(
                                                    crate::toast::ToastKind::Error,
                                                    format!("Token save failed: {err}"),
                                                );
                                            }
                                        }
                                    },
                                    if local_api_has_token { "Rotate token" } else { "Generate token" }
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| {
                                        let token = if !local_api_token_reveal().is_empty() {
                                            local_api_token_reveal()
                                        } else {
                                            crate::services::load_local_api_token(&crate::data_dir())
                                        };
                                        if token.is_empty() {
                                            crate::toast::push(
                                                crate::toast::ToastKind::Error,
                                                "No token to copy.",
                                            );
                                        } else {
                                            copy_text_to_clipboard(&token);
                                            crate::toast::push(
                                                crate::toast::ToastKind::Success,
                                                "Token copied.",
                                            );
                                        }
                                    },
                                    "Copy token"
                                }
                                Button {
                                    kind: ButtonKind::Danger,
                                    onclick: move |_| {
                                        crate::services::clear_local_api_token(&crate::data_dir());
                                        local_api_token_reveal.set(String::new());
                                        let _ = state().db.audit("local_api_token_clear", "", "");
                                        crate::toast::push(
                                            crate::toast::ToastKind::Success,
                                            "API token cleared — requests will 401 until you generate again.",
                                        );
                                    },
                                    "Clear token"
                                }
                            }
                        }

                        div { class: "mt-6 flex flex-col gap-3 border-t border-border/60 pt-4",
                            h3 { class: "text-sm font-medium text-foreground",
                                "Home Assistant MQTT discovery"
                            }
                            p { class: "text-sm text-muted-foreground",
                                "Separate from Weather MQTT. Publishes retained discovery configs and state every interval for companion online/state, enrolled count, and the first claimed repeater’s battery, noise, and probe time. Default off."
                            }
                            p { class: "text-xs text-muted-foreground", "{ha_mqtt_status}" }
                            ToggleSwitch {
                                checked: ha_mqtt_enabled(),
                                label: "Enable Home Assistant MQTT discovery".to_string(),
                                hint: Some("Requires a broker (Mosquitto). Discovery prefix must match HA MQTT integration.".to_string()),
                                onchange: move |next| {
                                    let prev = crate::services::setting_bool(
                                        &state().db,
                                        "ha_mqtt_enabled",
                                        false,
                                    );
                                    ha_mqtt_enabled.set(next);
                                    crate::services::set_setting_bool(
                                        &state().db,
                                        "ha_mqtt_enabled",
                                        next,
                                    );
                                    if prev != next {
                                        let _ = state().db.audit(
                                            if next { "ha_mqtt_enable" } else { "ha_mqtt_disable" },
                                            "",
                                            "",
                                        );
                                    }
                                    crate::services::bump_ha_mqtt_config();
                                }
                            }
                            div { class: "grid gap-2 sm:grid-cols-2",
                                div { class: "flex flex-col gap-1",
                                    label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Broker host" }
                                    TextInput {
                                        value: ha_mqtt_broker(),
                                        placeholder: "127.0.0.1".to_string(),
                                        oninput: move |event: FormEvent| {
                                            let value = event.value();
                                            ha_mqtt_broker.set(value.clone());
                                            let prev = state()
                                                .db
                                                .setting("ha_mqtt_broker")
                                                .ok()
                                                .flatten()
                                                .unwrap_or_default();
                                            let trimmed = value.trim().to_string();
                                            let _ = state().db.set_setting("ha_mqtt_broker", &trimmed);
                                            if prev != trimmed {
                                                let _ = state().db.audit("ha_mqtt_broker", "", &trimmed);
                                            }
                                            crate::services::bump_ha_mqtt_config();
                                        },
                                    }
                                }
                                div { class: "flex flex-col gap-1",
                                    label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Port" }
                                    TextInput {
                                        value: ha_mqtt_port(),
                                        placeholder: "1883".to_string(),
                                        oninput: move |event: FormEvent| {
                                            let value = event.value();
                                            ha_mqtt_port.set(value.clone());
                                            if value.trim().parse::<u16>().is_ok() {
                                                let _ = state().db.set_setting(
                                                    "ha_mqtt_port",
                                                    value.trim(),
                                                );
                                                crate::services::bump_ha_mqtt_config();
                                            }
                                        },
                                    }
                                }
                            }
                            ToggleSwitch {
                                checked: ha_mqtt_tls(),
                                disabled: !ha_mqtt_enabled(),
                                label: "TLS".to_string(),
                                onchange: move |next| {
                                    ha_mqtt_tls.set(next);
                                    crate::services::set_setting_bool(
                                        &state().db,
                                        "ha_mqtt_tls",
                                        next,
                                    );
                                    crate::services::bump_ha_mqtt_config();
                                }
                            }
                            div { class: "grid gap-2 sm:grid-cols-2",
                                div { class: "flex flex-col gap-1",
                                    label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Username (optional)" }
                                    TextInput {
                                        value: ha_mqtt_user(),
                                        placeholder: "".to_string(),
                                        oninput: move |event: FormEvent| {
                                            let value = event.value();
                                            ha_mqtt_user.set(value.clone());
                                            let _ = state().db.set_setting(
                                                "ha_mqtt_username",
                                                value.trim(),
                                            );
                                            crate::services::bump_ha_mqtt_config();
                                        },
                                    }
                                }
                                div { class: "flex flex-col gap-1",
                                    label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                        if ha_mqtt_pass_saved {
                                            "Password (saved in HQ data folder · leave blank to keep)"
                                        } else {
                                            "Password (saved as ha-mqtt.pass in HQ data folder · 0600)"
                                        }
                                    }
                                    TextInput {
                                        value: ha_mqtt_pass(),
                                        placeholder: "Broker password".to_string(),
                                        kind: "password".to_string(),
                                        oninput: move |event: FormEvent| ha_mqtt_pass.set(event.value()),
                                    }
                                    div { class: "flex gap-2",
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: move |_| {
                                                let pass = ha_mqtt_pass();
                                                match crate::services::save_ha_mqtt_password(
                                                    &crate::data_dir(),
                                                    &pass,
                                                ) {
                                                    Ok(()) => {
                                                        ha_mqtt_pass.set(String::new());
                                                        crate::toast::push(
                                                            crate::toast::ToastKind::Success,
                                                            if pass.trim().is_empty() {
                                                                "HA MQTT password cleared."
                                                            } else {
                                                                "HA MQTT password saved."
                                                            },
                                                        );
                                                    }
                                                    Err(err) => {
                                                        crate::toast::push(
                                                            crate::toast::ToastKind::Error,
                                                            format!("Password save failed: {err}"),
                                                        );
                                                    }
                                                }
                                            },
                                            "Save password"
                                        }
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: move |_| {
                                                crate::services::clear_ha_mqtt_password(&crate::data_dir());
                                                ha_mqtt_pass.set(String::new());
                                                crate::toast::push(
                                                    crate::toast::ToastKind::Success,
                                                    "HA MQTT password cleared.",
                                                );
                                            },
                                            "Clear"
                                        }
                                    }
                                }
                            }
                            div { class: "grid gap-2 sm:grid-cols-3",
                                div { class: "flex flex-col gap-1",
                                    label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Discovery prefix" }
                                    TextInput {
                                        value: ha_mqtt_discovery_prefix(),
                                        placeholder: "homeassistant".to_string(),
                                        oninput: move |event: FormEvent| {
                                            let value = event.value();
                                            ha_mqtt_discovery_prefix.set(value.clone());
                                            let trimmed = value.trim();
                                            let store = if trimmed.is_empty() {
                                                "homeassistant"
                                            } else {
                                                trimmed
                                            };
                                            let _ = state().db.set_setting(
                                                "ha_mqtt_discovery_prefix",
                                                store,
                                            );
                                            crate::services::bump_ha_mqtt_config();
                                        },
                                    }
                                }
                                div { class: "flex flex-col gap-1",
                                    label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Node id" }
                                    TextInput {
                                        value: ha_mqtt_node_id(),
                                        placeholder: "metis_mesh_hq".to_string(),
                                        oninput: move |event: FormEvent| {
                                            let value = event.value();
                                            ha_mqtt_node_id.set(value.clone());
                                            let trimmed = value.trim();
                                            let store = if trimmed.is_empty() {
                                                "metis_mesh_hq"
                                            } else {
                                                trimmed
                                            };
                                            let _ = state().db.set_setting("ha_mqtt_node_id", store);
                                            crate::services::bump_ha_mqtt_config();
                                        },
                                    }
                                }
                                div { class: "flex flex-col gap-1",
                                    label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "State prefix" }
                                    TextInput {
                                        value: ha_mqtt_state_prefix(),
                                        placeholder: "metis_mesh_hq".to_string(),
                                        oninput: move |event: FormEvent| {
                                            let value = event.value();
                                            ha_mqtt_state_prefix.set(value.clone());
                                            let trimmed = value.trim();
                                            let store = if trimmed.is_empty() {
                                                "metis_mesh_hq"
                                            } else {
                                                trimmed
                                            };
                                            let _ = state().db.set_setting(
                                                "ha_mqtt_state_prefix",
                                                store,
                                            );
                                            crate::services::bump_ha_mqtt_config();
                                        },
                                    }
                                }
                            }
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Publish interval (seconds)" }
                                TextInput {
                                    value: ha_mqtt_interval(),
                                    placeholder: "30".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        ha_mqtt_interval.set(value.clone());
                                        if let Ok(parsed) = value.trim().parse::<u64>() {
                                            let clamped =
                                                crate::services::clamp_ha_mqtt_interval(parsed);
                                            let _ = state().db.set_setting(
                                                "ha_mqtt_interval_secs",
                                                &clamped.to_string(),
                                            );
                                            ha_mqtt_interval.set(clamped.to_string());
                                            crate::services::bump_ha_mqtt_config();
                                        }
                                    },
                                }
                                span { class: "text-xs text-muted-foreground", "10–300 seconds (default 30)" }
                            }
                            div { class: "flex flex-wrap gap-2",
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| {
                                        let wx_broker = state()
                                            .db
                                            .setting("svc_weather_mqtt_broker")
                                            .ok()
                                            .flatten()
                                            .unwrap_or_default();
                                        let wx_port = state()
                                            .db
                                            .setting("svc_weather_mqtt_port")
                                            .ok()
                                            .flatten()
                                            .unwrap_or_else(|| "1883".into());
                                        let wx_tls = crate::services::setting_bool(
                                            &state().db,
                                            "svc_weather_mqtt_tls",
                                            false,
                                        );
                                        let wx_user = state()
                                            .db
                                            .setting("svc_weather_mqtt_username")
                                            .ok()
                                            .flatten()
                                            .unwrap_or_default();
                                        ha_mqtt_broker.set(wx_broker.clone());
                                        ha_mqtt_port.set(wx_port.clone());
                                        ha_mqtt_tls.set(wx_tls);
                                        ha_mqtt_user.set(wx_user.clone());
                                        let _ = state().db.set_setting("ha_mqtt_broker", wx_broker.trim());
                                        let _ = state().db.set_setting("ha_mqtt_port", wx_port.trim());
                                        crate::services::set_setting_bool(
                                            &state().db,
                                            "ha_mqtt_tls",
                                            wx_tls,
                                        );
                                        let _ = state()
                                            .db
                                            .set_setting("ha_mqtt_username", wx_user.trim());
                                        crate::services::bump_ha_mqtt_config();
                                        crate::toast::push(
                                            crate::toast::ToastKind::Success,
                                            "Copied broker/port/TLS/username from Weather MQTT. Password not copied — paste if needed.",
                                        );
                                    },
                                    "Fill from Weather MQTT"
                                }
                            }
                        }
                    }
                }

                if services_tab() == ServicesTab::Alerts {
                    div { class: "services-panel",
                        p { class: "text-sm text-muted-foreground",
                            "Relays post on {channel_info_name()}. Sourced from NWS CAP / IPAWS-style codes — labeled with time. Not a law-enforcement feed. Silent when nothing new."
                        }
                        ToggleSwitch {
                            checked: svc_nws(),
                            label: "Poll NWS alerts near site pin".to_string(),
                            hint: Some("US weather alerts. Silent if none new.".to_string()),
                            onchange: move |next| {
                                svc_nws.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_nws_enabled", next);
                            }
                        }
                        ToggleSwitch {
                            checked: svc_nws_auto(),
                            disabled: !svc_nws(),
                            label: "Auto-relay NWS to channel".to_string(),
                            hint: Some("Otherwise Inbox confirms before airtime.".to_string()),
                            onchange: move |next| {
                                svc_nws_auto.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_nws_auto_relay", next);
                            }
                        }
                        ToggleSwitch {
                            checked: svc_amber(),
                            label: "Amber alerts".to_string(),
                            hint: Some("NWS CAP CAE for the alert state — #amber / #amber+.".to_string()),
                            onchange: move |next| {
                                svc_amber.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_amber_enabled", next);
                            }
                        }
                        ToggleSwitch {
                            checked: svc_amber_auto(),
                            disabled: !svc_amber(),
                            label: "Auto-relay Amber to channel".to_string(),
                            hint: Some("Otherwise Inbox.".to_string()),
                            onchange: move |next| {
                                svc_amber_auto.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_amber_auto_relay", next);
                            }
                        }
                        ToggleSwitch {
                            checked: svc_emerg(),
                            label: "Civil emergencies".to_string(),
                            hint: Some("Evac, hazmat, LE, blue… — #alert / #alert+.".to_string()),
                            onchange: move |next| {
                                svc_emerg.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_emerg_enabled", next);
                            }
                        }
                        ToggleSwitch {
                            checked: svc_emerg_auto(),
                            disabled: !svc_emerg(),
                            label: "Auto-relay civil emergencies to channel".to_string(),
                            hint: Some("Otherwise Inbox.".to_string()),
                            onchange: move |next| {
                                svc_emerg_auto.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_emerg_auto_relay", next);
                            }
                        }
                        div { class: "w-28",
                            label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "Alert state (US)" }
                            TextInput {
                                value: svc_emerg_state(),
                                placeholder: "CA",
                                oninput: move |event: FormEvent| {
                                    let value = event.value().trim().to_ascii_uppercase();
                                    svc_emerg_state.set(value.clone());
                                    if value.len() == 2 && value.chars().all(|c| c.is_ascii_alphabetic()) {
                                        let _ = state().db.set_setting("svc_emerg_state", &value);
                                    }
                                },
                            }
                        }
                    }
                }

                if services_tab() == ServicesTab::Quake {
                    div { class: "services-panel",
                        ToggleSwitch {
                            checked: svc_quake(),
                            label: "Poll USGS quakes in HQ box".to_string(),
                            hint: Some("Posts only when a new quake is found. #quake on demand.".to_string()),
                            onchange: move |next| {
                                svc_quake.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_quake_enabled", next);
                            }
                        }
                        div { class: "w-40",
                            label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "Min magnitude" }
                            TextInput {
                                value: svc_quake_mag(),
                                placeholder: "3.0",
                                oninput: move |event: FormEvent| {
                                    let value = event.value();
                                    svc_quake_mag.set(value.clone());
                                    if value.parse::<f64>().is_ok() {
                                        let _ = state().db.set_setting("svc_quake_min_mag", &value);
                                    }
                                },
                            }
                        }
                    }
                }
            }
            } // end HqTab::Services

            if hq_tab() == HqTab::Preferences {
            Card { title: "Brand mark", icon: Some(IconName::Radio),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Top header logo (sidebar stays Metis Mesh HQ). Default is EmergHQ; upload PNG/JPG/WebP/GIF/SVG up to 2 MB to replace it. Clear restores the default."
                }
                {
                    let custom = crate::brand::custom_logo_url(&state().db);
                    rsx! {
                        if let Some(src) = custom {
                            img {
                                class: "mb-3 h-12 max-w-xs object-contain object-left rounded border border-border bg-background p-2",
                                src: "{src}",
                                alt: "Current brand logo",
                            }
                        } else {
                            img {
                                class: "mb-3 h-12 max-w-xs object-contain object-left rounded border border-border bg-background p-2",
                                src: "{crate::brand::default_logo_url()}",
                                alt: "Default EmergHQ logo",
                            }
                        }
                    }
                }
                div { class: "flex flex-wrap gap-2",
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| {
                            match crate::brand::pick_and_install_logo(&state().db) {
                                Ok(_) => status_note("Brand logo saved."),
                                Err(err) => status_note(err),
                            }
                        },
                        Icon { name: IconName::Check, size: 14 }
                        "Upload logo"
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        disabled: !crate::brand::has_custom_logo(&state().db),
                        onclick: move |_| {
                            match crate::brand::clear_logo(&state().db) {
                                Ok(()) => status_note("Restored default EmergHQ logo."),
                                Err(err) => status_note(err),
                            }
                        },
                        "Restore default"
                    }
                }
            }
            Card { title: "Private channels", icon: Some(IconName::Key),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Storage: {secret_label}. Approval sends both secrets; the message list shows [secret sent]. Revoke rotates both. Renaming keeps the same secret and re-sends the new name to enrolled nodes."
                }
                div { class: "mb-3 grid gap-3 sm:grid-cols-2",
                    div {
                        label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "Bot home (INFO)" }
                        TextInput {
                            value: channel_info_name(),
                            placeholder: crate::secrets::CHANNEL_NAME,
                            oninput: move |event: FormEvent| channel_info_name.set(event.value()),
                        }
                    }
                    div {
                        label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "Peer net (NET)" }
                        TextInput {
                            value: channel_net_name(),
                            placeholder: crate::secrets::NET_CHANNEL_NAME,
                            oninput: move |event: FormEvent| channel_net_name.set(event.value()),
                        }
                    }
                }
                div { class: "flex flex-wrap gap-2",
                    {
                        let radio_connected = radio.state == "connected";
                        rsx! {
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    let desired = channel_info_name();
                                    match state().db.rename_system_channel(
                                        SystemChannel::Info,
                                        &desired,
                                        &state().secrets,
                                    ) {
                                        Ok((name, slot)) => {
                                            channel_info_name.set(name.clone());
                                            if radio_connected {
                                                state().radio.push_channel(slot as u8, name.clone());
                                            }
                                            status_note(format!("Bot home channel is now {name}."));
                                        }
                                        Err(err) => {
                                            channel_info_name.set(
                                                state()
                                                    .db
                                                    .info_channel_name()
                                                    .unwrap_or_else(|_| crate::secrets::CHANNEL_NAME.into()),
                                            );
                                            status_note(err.to_string());
                                        }
                                    }
                                },
                                Icon { name: IconName::Check, size: 14 }
                                "Save bot home"
                            }
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    let desired = channel_net_name();
                                    match state().db.rename_system_channel(
                                        SystemChannel::Net,
                                        &desired,
                                        &state().secrets,
                                    ) {
                                        Ok((name, slot)) => {
                                            channel_net_name.set(name.clone());
                                            if radio_connected {
                                                state().radio.push_channel(slot as u8, name.clone());
                                            }
                                            status_note(format!("Peer net channel is now {name}."));
                                        }
                                        Err(err) => {
                                            channel_net_name.set(
                                                state()
                                                    .db
                                                    .net_channel_name()
                                                    .unwrap_or_else(|_| crate::secrets::NET_CHANNEL_NAME.into()),
                                            );
                                            status_note(err.to_string());
                                        }
                                    }
                                },
                                Icon { name: IconName::Check, size: 14 }
                                "Save peer net"
                            }
                        }
                    }
                }
            }
            AppearanceEditor {
                target: appearance_edit,
                on_saved: move |_| {},
            }
            Card { title: "Messages appearance".to_string(), icon: Some(IconName::Messages),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Global pane background, opacity, and text size for channels and DMs. Each channel can still override with Look (or the rail gear)."
                }
                Button {
                    kind: ButtonKind::Ghost,
                    onclick: move |_| {
                        appearance_edit.set(Some(AppearanceEditTarget::Global));
                    },
                    Icon { name: IconName::Settings, size: 14 }
                    "Edit appearance"
                }
            }
            Card { title: "History", icon: Some(IconName::Clock),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Default keep for channels and DMs (Channels page can override per channel). Messages auto-trim; use Purge to also clear old routes, locations, and telemetry. Roster, secrets, and audit stay."
                }
                div { class: "mb-3",
                    MessageKeepButton {
                        current: message_keep(),
                        onpick: move |value: String| {
                            let _ = state().db.set_message_keep(&value);
                            message_keep.set(value.clone());
                            if value != "forever" {
                                let _ = state().db.apply_message_keep();
                            }
                        },
                    }
                }
                div { class: "flex gap-2",
                    Button {
                        kind: ButtonKind::Danger,
                        disabled: message_keep() == "forever",
                        onclick: move |_| confirm_purge.set(true),
                        Icon { name: IconName::Trash, size: 14 }
                        "Purge older data"
                    }
                }
                if !purge_note().is_empty() {
                    p { class: "mt-2 text-sm text-muted-foreground", "{purge_note}" }
                }
            }
            Card { title: "Claimed infrastructure".to_string(), icon: Some(IconName::Signal),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "LoRa status probes for verified claimed repeaters / rooms / sensors (Overview → Repeaters). Probes use airtime — keep intervals modest. Admin passwords are never stored."
                }
                p { class: "mb-2 text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground",
                    "Auto-probe while companion Ready"
                }
                div { class: "mb-3 flex flex-wrap gap-2",
                    RetentionChoice { label: "Off (connect only)", value: "0", current: infra_probe_interval(), onpick: move |value: String| {
                        let mins: i64 = value.parse().unwrap_or(0);
                        let _ = state().db.set_infra_probe_interval_mins(mins);
                        infra_probe_interval.set(state().db.infra_probe_interval_mins().to_string());
                    } }
                    RetentionChoice { label: "15 min", value: "15", current: infra_probe_interval(), onpick: move |value: String| {
                        let mins: i64 = value.parse().unwrap_or(15);
                        let _ = state().db.set_infra_probe_interval_mins(mins);
                        infra_probe_interval.set(state().db.infra_probe_interval_mins().to_string());
                    } }
                    RetentionChoice { label: "30 min", value: "30", current: infra_probe_interval(), onpick: move |value: String| {
                        let mins: i64 = value.parse().unwrap_or(30);
                        let _ = state().db.set_infra_probe_interval_mins(mins);
                        infra_probe_interval.set(state().db.infra_probe_interval_mins().to_string());
                    } }
                    RetentionChoice { label: "60 min", value: "60", current: infra_probe_interval(), onpick: move |value: String| {
                        let mins: i64 = value.parse().unwrap_or(60);
                        let _ = state().db.set_infra_probe_interval_mins(mins);
                        infra_probe_interval.set(state().db.infra_probe_interval_mins().to_string());
                    } }
                }
                div { class: "mb-3 grid gap-3 sm:grid-cols-2",
                    div {
                        label {
                            class: "mb-1 block text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground",
                            r#for: "infra-probe-max",
                            "Max nodes per sweep"
                        }
                        SelectInput {
                            id: "infra-probe-max".to_string(),
                            value: infra_probe_max(),
                            onchange: move |evt: FormEvent| {
                                let n: i64 = evt.value().parse().unwrap_or(5);
                                let _ = state().db.set_infra_probe_max_targets(n);
                                infra_probe_max.set(state().db.infra_probe_max_targets().to_string());
                            },
                            for n in 1..=10i64 {
                                option { value: "{n}", "{n}" }
                            }
                        }
                    }
                    div {
                        label {
                            class: "mb-1 block text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground",
                            r#for: "infra-status-cap",
                            "Samples kept per node"
                        }
                        SelectInput {
                            id: "infra-status-cap".to_string(),
                            value: infra_status_cap(),
                            onchange: move |evt: FormEvent| {
                                let n: i64 = evt.value().parse().unwrap_or(500);
                                let _ = state().db.set_infra_status_sample_cap(n);
                                infra_status_cap.set(state().db.infra_status_sample_cap().to_string());
                                let _ = state().db.apply_infra_status_keep();
                            },
                            option { value: "100", "100" }
                            option { value: "250", "250" }
                            option { value: "500", "500" }
                            option { value: "1000", "1000" }
                            option { value: "2000", "2000" }
                        }
                    }
                }
                p { class: "mb-2 text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground",
                    "Status sample retention"
                }
                div { class: "mb-3 flex flex-wrap gap-2",
                    RetentionChoice { label: "7 days", value: "7d", current: infra_status_keep(), onpick: move |value: String| {
                        let _ = state().db.set_infra_status_keep(&value);
                        infra_status_keep.set(value);
                        let _ = state().db.apply_infra_status_keep();
                    } }
                    RetentionChoice { label: "30 days", value: "30d", current: infra_status_keep(), onpick: move |value: String| {
                        let _ = state().db.set_infra_status_keep(&value);
                        infra_status_keep.set(value);
                        let _ = state().db.apply_infra_status_keep();
                    } }
                    RetentionChoice { label: "90 days", value: "90d", current: infra_status_keep(), onpick: move |value: String| {
                        let _ = state().db.set_infra_status_keep(&value);
                        infra_status_keep.set(value);
                        let _ = state().db.apply_infra_status_keep();
                    } }
                    RetentionChoice { label: "Forever", value: "forever", current: infra_status_keep(), onpick: move |value: String| {
                        let _ = state().db.set_infra_status_keep(&value);
                        infra_status_keep.set(value);
                    } }
                }
                div { class: "flex flex-wrap gap-2",
                    Button {
                        kind: ButtonKind::Danger,
                        disabled: infra_status_keep() == "forever",
                        onclick: move |_| confirm_infra_purge.set(true),
                        Icon { name: IconName::Trash, size: 14 }
                        "Purge older status samples"
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| {
                            match state().db.apply_infra_status_keep() {
                                Ok(n) => infra_purge_note.set(format!(
                                    "Trimmed {n} sample row(s) to current keep/cap."
                                )),
                                Err(err) => infra_purge_note.set(err.to_string()),
                            }
                        },
                        "Apply keep / cap now"
                    }
                }
                if !infra_purge_note().is_empty() {
                    p { class: "mt-2 text-sm text-muted-foreground", "{infra_purge_note}" }
                }
            }
            } // end HqTab::Preferences (secret + history)

            if hq_tab() == HqTab::Data {
            {
                let _ = regions_tick();
                let _ = map_upload_tick();
                let map_status = crate::services::map_upload_status_line(&state().db);
                let regions = crate::map_tiles::list_installed_regions(&state().db);
                let active = crate::map_tiles::active_region_id(&state().db).unwrap_or_default();
                let dl = crate::map_tiles::download_status();
                rsx! {
                    Card { title: "Community map upload", icon: Some(IconName::Map),
                        p { class: "mb-3 text-sm text-muted-foreground",
                            "Optional. When enabled and authorized, HQ posts overheard repeater / room / sensor ADVERTs that include GPS to map.meshcore.io. Companions, Tracker trails, and HQ site pins stay local. Needs internet. Signing uses the primary companion — the private key is never stored in HQ."
                        }
                        ToggleSwitch {
                            checked: map_upload_enabled(),
                            label: "Upload infra GPS adverts".to_string(),
                            hint: Some(map_status.clone()),
                            onchange: move |next| {
                                map_upload_enabled.set(next);
                                crate::services::set_setting_bool(
                                    &state().db,
                                    "svc_map_upload_enabled",
                                    next,
                                );
                                if !next {
                                    crate::services::revoke_map_upload_auth(&state().db);
                                    map_upload_authorized.set(false);
                                }
                                map_upload_tick.with_mut(|n| *n = n.wrapping_add(1));
                            }
                        }
                        div { class: "mb-3 flex flex-wrap items-center gap-2",
                            Button {
                                kind: ButtonKind::Primary,
                                disabled: !map_upload_enabled() || map_upload_authorized(),
                                onclick: move |_| {
                                    let radio = state().radio.clone();
                                    let db = state().db.clone();
                                    map_upload_note.set("Authorizing…".into());
                                    spawn(async move {
                                        match crate::services::authorize_map_upload(&db, &radio).await {
                                            Ok(()) => {
                                                map_upload_authorized.set(true);
                                                map_upload_note.set(
                                                    "Authorized. Primary companion will sign uploads.".into(),
                                                );
                                                crate::toast::push(
                                                    crate::toast::ToastKind::Success,
                                                    "Map upload authorized",
                                                );
                                            }
                                            Err(err) => {
                                                map_upload_authorized.set(false);
                                                map_upload_note.set(err.clone());
                                                crate::toast::push(
                                                    crate::toast::ToastKind::Error,
                                                    err,
                                                );
                                            }
                                        }
                                        map_upload_tick.with_mut(|n| *n = n.wrapping_add(1));
                                    });
                                },
                                if map_upload_authorized() { "Authorized" } else { "Authorize" }
                            }
                            if map_upload_authorized() {
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| {
                                        crate::services::revoke_map_upload_auth(&state().db);
                                        map_upload_authorized.set(false);
                                        map_upload_note.set("Authorization cleared.".into());
                                        map_upload_tick.with_mut(|n| *n = n.wrapping_add(1));
                                    },
                                    "Revoke"
                                }
                            }
                        }
                        p { class: "mb-3 text-xs text-muted-foreground",
                            "Authorize once while the primary companion is connected. Private key stays on the radio (or briefly in RAM if firmware cannot sign)."
                        }
                        ToggleSwitch {
                            checked: map_upload_dry_run(),
                            disabled: !map_upload_enabled(),
                            label: "Dry run (log only, no POST)".to_string(),
                            onchange: move |next| {
                                map_upload_dry_run.set(next);
                                crate::services::set_setting_bool(
                                    &state().db,
                                    "svc_map_upload_dry_run",
                                    next,
                                );
                                map_upload_tick.with_mut(|n| *n = n.wrapping_add(1));
                            }
                        }
                        div { class: "mb-3 grid gap-2 sm:grid-cols-2",
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "Min re-upload interval (seconds)" }
                                TextInput {
                                    value: map_upload_interval(),
                                    placeholder: "3600".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        map_upload_interval.set(value.clone());
                                        if let Ok(secs) = value.trim().parse::<i64>() {
                                            if (60..7 * 24 * 3600).contains(&secs) {
                                                let _ = state().db.set_setting(
                                                    "svc_map_upload_interval_secs",
                                                    value.trim(),
                                                );
                                            }
                                        }
                                    },
                                }
                            }
                            div { class: "flex flex-col gap-1",
                                label { class: "text-xs uppercase tracking-[0.16em] text-muted-foreground", "API URL" }
                                TextInput {
                                    value: map_upload_api(),
                                    placeholder: "https://map.meshcore.io/api/v1/uploader/node".to_string(),
                                    oninput: move |event: FormEvent| {
                                        let value = event.value();
                                        map_upload_api.set(value.clone());
                                        let trimmed = value.trim();
                                        if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
                                            let fixed =
                                                crate::services::map_upload::normalize_map_api_url(
                                                    trimmed,
                                                );
                                            let _ = state().db.set_setting(
                                                "svc_map_upload_api_url",
                                                &fixed,
                                            );
                                            if fixed != trimmed {
                                                map_upload_api.set(fixed);
                                            }
                                        }
                                    },
                                }
                            }
                        }
                        if !map_upload_note().is_empty() {
                            p { class: "text-sm text-muted-foreground", "{map_upload_note()}" }
                        }
                    }
                    Card { title: "Offline map packs", icon: Some(IconName::Map),
                        p { class: "mb-3 text-sm text-muted-foreground",
                            "Installed packs only. Download new areas from Map (ZIP + radius). One pack covers Streets, Terrain, and Topo."
                        }
                        if dl.running || (dl.phase == "error" && !dl.detail.is_empty()) {
                            p { class: "mb-2 text-xs text-muted-foreground",
                                if dl.running {
                                    "Downloading {dl.region_id}: {dl.detail}"
                                } else {
                                    "Last error: {dl.detail}"
                                }
                            }
                        }
                        if regions.is_empty() {
                            p { class: "text-sm text-muted-foreground",
                                "No packs yet. Open Map and use Download offline map."
                            }
                        }
                        div { class: "flex flex-col gap-2",
                            for region in regions.iter() {
                                {
                                    let id = region.id.clone();
                                    let id_active = id.clone();
                                    let id_dl = id.clone();
                                    let id_del = id.clone();
                                    let is_active = active == region.id;
                                    let status = region.status.clone();
                                    let name = region.name.clone();
                                    let bytes = region.bytes;
                                    let bbox = format!(
                                        "{:.2},{:.2} → {:.2},{:.2}",
                                        region.bbox[0], region.bbox[1], region.bbox[2], region.bbox[3]
                                    );
                                    rsx! {
                                        div {
                                            key: "{id}",
                                            class: "flex flex-wrap items-center gap-2 rounded-md border border-white/10 bg-[#151a20] px-3 py-2",
                                            label { class: "flex min-w-[10rem] flex-1 items-center gap-2 text-sm",
                                                input {
                                                    r#type: "radio",
                                                    name: "map-region-active",
                                                    checked: is_active,
                                                    oninput: move |_| {
                                                        let _ = crate::map_tiles::set_active_region(&state().db, &id_active);
                                                        regions_tick.with_mut(|n| *n = n.wrapping_add(1));
                                                    }
                                                }
                                                span { class: "font-medium", "{name}" }
                                            }
                                            span { class: "font-mono text-[0.65rem] text-muted-foreground", "{bbox}" }
                                            span { class: "text-[0.65rem] text-muted-foreground",
                                                "{status}"
                                                if bytes > 0 {
                                                    " · {bytes / 1_000_000} MB"
                                                }
                                            }
                                            Button {
                                                kind: ButtonKind::Ghost,
                                                disabled: dl.running,
                                                onclick: move |_| {
                                                    crate::map_tiles::start_download(state().db.clone(), id_dl.clone());
                                                    regions_tick.with_mut(|n| *n = n.wrapping_add(1));
                                                    spawn(async move {
                                                        for _ in 0..120 {
                                                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                                            regions_tick.with_mut(|n| *n = n.wrapping_add(1));
                                                            if !crate::map_tiles::download_status().running {
                                                                break;
                                                            }
                                                        }
                                                    });
                                                },
                                                "Update"
                                            }
                                            Button {
                                                kind: ButtonKind::Ghost,
                                                disabled: dl.running,
                                                onclick: move |_| {
                                                    let _ = crate::map_tiles::delete_region(&state().db, &id_del);
                                                    regions_tick.with_mut(|n| *n = n.wrapping_add(1));
                                                },
                                                Icon { name: IconName::Trash, size: 14 }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            Card { title: "Backup & restore", icon: Some(IconName::Key),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Encrypted .metisbak archive (passphrase required). Includes the database and private channel secrets. Restore replaces this HQ’s state."
                }
                div { class: "mb-3 grid gap-2 sm:grid-cols-2",
                    div {
                        label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "Passphrase" }
                        TextInput {
                            value: backup_pass(),
                            placeholder: "Passphrase",
                            kind: "password".to_string(),
                            oninput: move |event: FormEvent| backup_pass.set(event.value()),
                        }
                    }
                    div {
                        label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "Confirm" }
                        TextInput {
                            value: backup_pass2(),
                            placeholder: "Confirm passphrase",
                            kind: "password".to_string(),
                            oninput: move |event: FormEvent| backup_pass2.set(event.value()),
                        }
                    }
                }
                {
                    let a = backup_pass();
                    let b = backup_pass2();
                    let pass_ok = !a.trim().is_empty() && a == b;
                    let pass_mismatch = !a.is_empty() && !b.is_empty() && a != b;
                    rsx! {
                        if pass_mismatch {
                            p { class: "mb-2 text-sm text-amber-200", "Passphrases do not match." }
                        } else if a.trim().is_empty() {
                            p { class: "mb-2 text-sm text-muted-foreground", "Enter a passphrase to create a backup or enable a schedule." }
                        }
                        div { class: "mb-3 flex flex-wrap gap-2",
                            Button {
                                kind: ButtonKind::Primary,
                                disabled: !pass_ok,
                                onclick: move |_| {
                                    let a = backup_pass();
                                    let b = backup_pass2();
                                    if a.trim().is_empty() {
                                        let msg = "Passphrase required to create a backup.";
                                        backup_note.set(msg.into());
                                        crate::toast::push(crate::toast::ToastKind::Error, msg);
                                        return;
                                    }
                                    if a != b {
                                        let msg = "Passphrases do not match.";
                                        backup_note.set(msg.into());
                                        crate::toast::push(crate::toast::ToastKind::Error, msg);
                                        return;
                                    }
                                    let path = crate::backup::default_backup_path(&crate::data_dir());
                                    match crate::backup::write_backup_file(
                                        &state().db,
                                        &state().secrets,
                                        a.trim(),
                                        &path,
                                    ) {
                                        Ok(saved) => {
                                            let _ = state().db.audit(
                                                "backup",
                                                "",
                                                &saved.display().to_string(),
                                            );
                                            backup_note.set(format!("Saved {}", saved.display()));
                                            status_note(format!("Backup saved to {}", saved.display()));
                                            restore_path.set(saved.display().to_string());
                                            backups_tick.with_mut(|n| *n = n.wrapping_add(1));
                                            backup_pass.set(String::new());
                                            backup_pass2.set(String::new());
                                        }
                                        Err(err) => {
                                            backup_note.set(err.to_string());
                                            crate::toast::push(crate::toast::ToastKind::Error, err.to_string());
                                        }
                                    }
                                },
                                "Create backup"
                            }
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    backups_tick.with_mut(|n| *n = n.wrapping_add(1));
                                    download_backups_open.set(true);
                                },
                                Icon { name: IconName::Copy, size: 14 }
                                "Download / manage…"
                            }
                        }
                        div { class: "mb-4 rounded-md border border-border bg-background/50 p-3",
                            label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "Schedule" }
                            p { class: "mb-2 text-xs text-muted-foreground",
                                "Automatic backups need a passphrase saved in the HQ data folder (0600). Changing the dropdown alone does nothing — enter matching passphrases above, then Save schedule."
                            }
                            p { class: "mb-2 text-xs text-muted-foreground",
                                "Currently saved: {backup_schedule_saved()}"
                            }
                            if backup_schedule() != backup_schedule_saved() {
                                p { class: "mb-2 text-xs text-amber-200",
                                    "Draft change not saved yet."
                                }
                            }
                            div { class: "mb-2",
                                SelectInput {
                                    id: "backup-schedule".to_string(),
                                    value: backup_schedule(),
                                    onchange: move |event: FormEvent| backup_schedule.set(event.value()),
                                    option { value: "off", style: "background-color:#151a20;color:#f4f4f5;", "Off" }
                                    option { value: "daily", style: "background-color:#151a20;color:#f4f4f5;", "Once a day" }
                                    option { value: "weekly", style: "background-color:#151a20;color:#f4f4f5;", "Once a week" }
                                    option { value: "monthly", style: "background-color:#151a20;color:#f4f4f5;", "Once a month" }
                                }
                            }
                            Button {
                                kind: ButtonKind::Ghost,
                                disabled: backup_schedule() != "off" && !pass_ok,
                                onclick: move |_| {
                                    let schedule = backup_schedule();
                                    if schedule == "off" {
                                        let _ = state().db.set_setting("backup_schedule", "off");
                                        crate::backup::clear_schedule_passphrase(&crate::data_dir());
                                        backup_schedule_saved.set("off".into());
                                        backup_note.set("Scheduled backups off.".into());
                                        status_note("Scheduled backups off");
                                        return;
                                    }
                                    let a = backup_pass();
                                    let b = backup_pass2();
                                    if a.trim().is_empty() || a != b {
                                        let msg = "Enter matching passphrase fields to save the schedule.";
                                        backup_note.set(msg.into());
                                        crate::toast::push(crate::toast::ToastKind::Error, msg);
                                        backup_schedule.set(backup_schedule_saved());
                                        return;
                                    }
                                    match crate::backup::save_schedule_passphrase(&crate::data_dir(), a.trim()) {
                                        Ok(()) => {
                                            let _ = state().db.set_setting("backup_schedule", &schedule);
                                            backup_schedule_saved.set(schedule.clone());
                                            backup_note.set(format!("Schedule saved ({schedule})."));
                                            status_note(format!("Backup schedule: {schedule}"));
                                            backup_pass.set(String::new());
                                            backup_pass2.set(String::new());
                                        }
                                        Err(err) => {
                                            backup_note.set(err.to_string());
                                            crate::toast::push(crate::toast::ToastKind::Error, err.to_string());
                                            backup_schedule.set(backup_schedule_saved());
                                        }
                                    }
                                },
                                "Save schedule"
                            }
                        }
                    }
                }
                div { class: "backup-restore-section",
                span { class: "backup-restore-pill", "Restore" }
                {
                    let _ = backups_tick();
                    let backups = crate::backup::list_backup_entries(&crate::data_dir());
                    let selected = restore_path();
                    let select_value = if backups.iter().any(|b| b.path_string() == selected) {
                        selected
                    } else {
                        String::new()
                    };
                    rsx! {
                        if backups.is_empty() {
                            p { class: "mb-3 text-sm text-muted-foreground",
                                "No backups yet. Create one above, or open a .metisbak file."
                            }
                        } else {
                            div { class: "mb-3",
                                label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "Backup" }
                                SelectInput {
                                    id: "restore-backup-select".to_string(),
                                    value: select_value,
                                    onchange: move |event: FormEvent| restore_path.set(event.value()),
                                    option {
                                        value: "",
                                        style: "background-color:#151a20;color:#f4f4f5;",
                                        "Choose a backup…"
                                    }
                                    for entry in backups.iter() {
                                        {
                                            let path_s = entry.path_string();
                                            let when = if entry.created_at > 0 {
                                                timefmt::format_stamp(&state().db, entry.created_at)
                                            } else {
                                                entry.file_name.clone()
                                            };
                                            let label = format!("{when} · {}", entry.file_name);
                                            rsx! {
                                                option {
                                                    key: "{path_s}",
                                                    value: "{path_s}",
                                                    style: "background-color:#151a20;color:#f4f4f5;",
                                                    "{label}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "mb-3",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    match crate::backup::import_backup_with_dialog(&crate::data_dir()) {
                                        Ok(dest) => {
                                            let shown = dest.display().to_string();
                                            let _ = state().db.audit("backup_import", "", &shown);
                                            restore_path.set(shown.clone());
                                            backups_tick.with_mut(|n| *n = n.wrapping_add(1));
                                            backup_note.set(format!("Opened {shown}"));
                                            status_note("Backup file opened for restore");
                                        }
                                        Err(err) => {
                                            let msg = err.to_string();
                                            if msg != "Open cancelled." {
                                                backup_note.set(msg.clone());
                                                crate::toast::push(crate::toast::ToastKind::Error, msg);
                                            }
                                        }
                                    }
                                },
                                "Open backup file…"
                            }
                        }
                    }
                }
                div { class: "mb-3",
                    label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "Passphrase" }
                    TextInput {
                        value: restore_pass(),
                        placeholder: "Passphrase for selected backup",
                        kind: "password".to_string(),
                        oninput: move |event: FormEvent| restore_pass.set(event.value()),
                    }
                }
                div { class: "flex flex-wrap gap-2",
                    Button {
                        kind: ButtonKind::Danger,
                        onclick: move |_| {
                            let path = restore_path();
                            let pass = restore_pass();
                            if path.trim().is_empty() || pass.trim().is_empty() {
                                let msg = "Choose a backup and enter its passphrase.";
                                backup_note.set(msg.into());
                                crate::toast::push(crate::toast::ToastKind::Error, msg);
                                return;
                            }
                            match crate::backup::decrypt_backup_file(
                                std::path::Path::new(path.trim()),
                                pass.trim(),
                            ) {
                                Ok(payload) => match crate::backup::preview_backup(&payload) {
                                    Ok(preview) => {
                                        restore_preview.set(Some((path, pass, preview)));
                                    }
                                    Err(err) => {
                                        backup_note.set(err.to_string());
                                        crate::toast::push(crate::toast::ToastKind::Error, err.to_string());
                                    }
                                },
                                Err(err) => {
                                    backup_note.set(err.to_string());
                                    crate::toast::push(crate::toast::ToastKind::Error, err.to_string());
                                }
                            }
                        },
                        "Preview restore…"
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| {
                            let path = restore_path();
                            if path.trim().is_empty() {
                                let msg = "Choose a backup to delete.";
                                backup_note.set(msg.into());
                                crate::toast::push(crate::toast::ToastKind::Error, msg);
                                return;
                            }
                            match crate::backup::delete_backup(std::path::Path::new(path.trim())) {
                                Ok(()) => {
                                    let _ = state().db.audit("backup_delete", "", path.trim());
                                    backup_note.set(format!("Deleted {path}"));
                                    status_note("Backup deleted");
                                    restore_path.set(String::new());
                                    backups_tick.with_mut(|n| *n = n.wrapping_add(1));
                                }
                                Err(err) => {
                                    backup_note.set(err.to_string());
                                    crate::toast::push(crate::toast::ToastKind::Error, err.to_string());
                                }
                            }
                        },
                        Icon { name: IconName::Trash, size: 14 }
                        "Delete selected"
                    }
                }
                }
                if !backup_note().is_empty() {
                    p { class: "mt-4 text-sm text-muted-foreground", "{backup_note}" }
                }
            }
            Card { title: "Contacts export / import", icon: Some(IconName::Users),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Bulk roster JSON (pubkey, name, type, optional GPS). Import merges into HQ only — it does not push contacts onto the radio."
                }
                div { class: "mb-3 flex flex-wrap gap-2",
                    Button {
                        kind: ButtonKind::Primary,
                        onclick: move |_| {
                            match crate::contact_export::create_timestamped_export(
                                &state().db,
                                &crate::data_dir(),
                            ) {
                                Ok((json_path, n)) => {
                                    let _ = state().db.audit(
                                        "contacts_export",
                                        "",
                                        &format!("{n} → {}", json_path.display()),
                                    );
                                    contacts_import_path.set(json_path.display().to_string());
                                    contacts_note.set(format!(
                                        "Exported {n} contacts to {}.",
                                        json_path.display()
                                    ));
                                    status_note(format!("Exported {n} contacts"));
                                    exports_tick.with_mut(|n| *n = n.wrapping_add(1));
                                }
                                Err(err) => contacts_note.set(err),
                            }
                        },
                        "Export contacts"
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| {
                            exports_tick.with_mut(|n| *n = n.wrapping_add(1));
                            download_exports_open.set(true);
                        },
                        Icon { name: IconName::Copy, size: 14 }
                        "Download / manage…"
                    }
                }
                div { class: "backup-restore-section",
                span { class: "backup-restore-pill backup-restore-pill-import", "Import" }
                {
                    let _ = exports_tick();
                    let exports = crate::contact_export::list_contact_exports(&crate::data_dir());
                    let selected = contacts_import_path();
                    let select_value = if exports.iter().any(|e| e.path_string() == selected) {
                        selected
                    } else {
                        String::new()
                    };
                    rsx! {
                        if exports.is_empty() {
                            p { class: "mb-3 text-sm text-muted-foreground",
                                "No contact exports yet. Export above, or open a contacts JSON file."
                            }
                        } else {
                            div { class: "mb-3",
                                label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "Export file" }
                                SelectInput {
                                    id: "contacts-import-select".to_string(),
                                    value: select_value,
                                    onchange: move |event: FormEvent| contacts_import_path.set(event.value()),
                                    option {
                                        value: "",
                                        style: "background-color:#151a20;color:#f4f4f5;",
                                        "Choose an export…"
                                    }
                                    for entry in exports.iter() {
                                        {
                                            let path_s = entry.path_string();
                                            let when = if entry.created_at > 0 {
                                                timefmt::format_stamp(&state().db, entry.created_at)
                                            } else {
                                                entry.file_name.clone()
                                            };
                                            let label = format!("{when} · {}", entry.file_name);
                                            rsx! {
                                                option {
                                                    key: "{path_s}",
                                                    value: "{path_s}",
                                                    style: "background-color:#151a20;color:#f4f4f5;",
                                                    "{label}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "mb-3",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    match crate::contact_export::import_contacts_with_dialog(
                                        &crate::data_dir(),
                                    ) {
                                        Ok(dest) => {
                                            let shown = dest.display().to_string();
                                            let _ = state().db.audit("contacts_import_open", "", &shown);
                                            contacts_import_path.set(shown.clone());
                                            exports_tick.with_mut(|n| *n = n.wrapping_add(1));
                                            contacts_note.set(format!("Opened {shown}"));
                                            status_note("Contacts file opened for import");
                                        }
                                        Err(err) => {
                                            if err != "Open cancelled." {
                                                contacts_note.set(err.clone());
                                                crate::toast::push(crate::toast::ToastKind::Error, err);
                                            }
                                        }
                                    }
                                },
                                "Open contacts file…"
                            }
                        }
                    }
                }
                div { class: "flex flex-wrap gap-2",
                    Button {
                        kind: ButtonKind::Primary,
                        onclick: move |_| {
                            let path = contacts_import_path();
                            if path.trim().is_empty() {
                                let msg = "Choose or open a contacts export first.";
                                contacts_note.set(msg.into());
                                crate::toast::push(crate::toast::ToastKind::Error, msg);
                                return;
                            }
                            match crate::contact_export::read_contacts_file(std::path::Path::new(
                                path.trim(),
                            )) {
                                Ok(file) => {
                                    contacts_import_preview.set(crate::contact_export::preview_import(&file));
                                    confirm_contacts_import.set(true);
                                }
                                Err(err) => {
                                    contacts_note.set(err.clone());
                                    crate::toast::push(crate::toast::ToastKind::Error, err);
                                }
                            }
                        },
                        "Import selected…"
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| {
                            let path = contacts_import_path();
                            if path.trim().is_empty() {
                                let msg = "Choose an export to delete.";
                                contacts_note.set(msg.into());
                                crate::toast::push(crate::toast::ToastKind::Error, msg);
                                return;
                            }
                            match crate::contact_export::delete_contact_export(
                                std::path::Path::new(path.trim()),
                            ) {
                                Ok(()) => {
                                    let _ = state().db.audit("contacts_export_delete", "", path.trim());
                                    contacts_note.set(format!("Deleted {path}"));
                                    status_note("Contact export deleted");
                                    contacts_import_path.set(String::new());
                                    exports_tick.with_mut(|n| *n = n.wrapping_add(1));
                                }
                                Err(err) => {
                                    contacts_note.set(err.clone());
                                    crate::toast::push(crate::toast::ToastKind::Error, err);
                                }
                            }
                        },
                        Icon { name: IconName::Trash, size: 14 }
                        "Delete selected"
                    }
                }
                }
                if !contacts_note().is_empty() {
                    p { class: "mt-4 text-sm text-muted-foreground", "{contacts_note}" }
                }
            }
            } // end HqTab::Data
            } // end hq-settings-panel
            } // end HQ tab

            if settings_tab() == SettingsTab::Device {
                if !linked {
                    Card { title: "Companion device".to_string(), icon: Some(IconName::Radio),
                        p { class: "mb-3 text-sm text-muted-foreground",
                            "Connect a companion from the HQ tab to edit MeshCore identity, radio presets, network scope, and firmware vars."
                        }
                        Button {
                            onclick: move |_| settings_tab.set(SettingsTab::Hq),
                            Icon { name: IconName::Bluetooth, size: 14 }
                            "Go to HQ pairing"
                        }
                    }
                } else {
                    div { class: "settings-subtabs",
                        button {
                            class: if device_tab() == DeviceTab::Identity { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                            onclick: move |_| device_tab.set(DeviceTab::Identity),
                            Icon { name: IconName::Radio, size: 14 }
                            "Identity"
                        }
                        button {
                            class: if device_tab() == DeviceTab::Radio { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                            onclick: move |_| device_tab.set(DeviceTab::Radio),
                            Icon { name: IconName::Signal, size: 14 }
                            "Radio"
                        }
                        button {
                            class: if device_tab() == DeviceTab::Network { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                            onclick: move |_| device_tab.set(DeviceTab::Network),
                            Icon { name: IconName::Locate, size: 14 }
                            "Network"
                        }
                        button {
                            class: if device_tab() == DeviceTab::Advanced { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                            onclick: move |_| device_tab.set(DeviceTab::Advanced),
                            Icon { name: IconName::Settings, size: 14 }
                            "Advanced"
                        }
                    }

                    div { class: "device-settings-panel",
                    if device_tab() == DeviceTab::Identity {
                        Card { title: "Public info".to_string(), icon: Some(IconName::Radio),
                            p { class: "mb-3 text-sm text-muted-foreground",
                                "Name and location other MeshCore nodes see in adverts."
                            }
                            div { class: "mb-3 flex gap-2",
                                TextInput {
                                    value: hq_name(),
                                    placeholder: "Metis HQ",
                                    oninput: move |event: FormEvent| hq_name.set(event.value()),
                                }
                                Button {
                                    onclick: move |_| {
                                        let _ = state().db.set_setting("hq_name", &hq_name());
                                        state().radio.apply_name(hq_name());
                                    },
                                    Icon { name: IconName::Check, size: 14 }
                                    "Apply name"
                                }
                            }
                            p { class: "mb-1 text-xs uppercase tracking-[0.14em] text-muted-foreground", "Public key" }
                            p { class: "mb-3 break-all font-mono text-xs text-muted-foreground",
                                if radio.public_key.is_empty() { "—" } else { "{radio.public_key}" }
                            }
                            if !radio.public_key.is_empty() {
                                {
                                    let hq_label = if hq_name().trim().is_empty() {
                                        "Metis HQ".to_string()
                                    } else {
                                        hq_name()
                                    };
                                    let uri = contact_add_uri(&hq_label, &radio.public_key, 1);
                                    let hq_row = state()
                                        .db
                                        .qr_contact_row(&radio.public_key)
                                        .ok()
                                        .flatten();
                                    let qr = hq_row
                                        .as_ref()
                                        .and_then(|row| row.qr_file.as_deref())
                                        .and_then(qr_data_url_from_file)
                                        .or_else(|| {
                                            hq_row
                                                .as_ref()
                                                .and_then(|row| row.qr_file.as_deref())
                                                .and_then(qr_public_url)
                                        })
                                        .unwrap_or_default();
                                    if qr.is_empty() {
                                        enqueue_contact_qr(&radio.public_key);
                                    }
                                    let uri_copy = uri.clone();
                                    let qr_print = qr.clone();
                                    let name_print = hq_label.clone();
                                    let key_print = radio.public_key.clone();
                                    rsx! {
                                        div { class: "mb-4 flex flex-wrap items-start gap-4",
                                            if !qr.is_empty() {
                                                div {
                                                    class: "shrink-0 overflow-hidden rounded-md border border-border bg-white p-1",
                                                    style: "width:7.5rem;height:7.5rem;",
                                                    img {
                                                        src: "{qr}",
                                                        alt: "HQ contact QR",
                                                        class: "h-full w-full",
                                                    }
                                                }
                                            } else {
                                                p { class: "text-xs text-muted-foreground", "Generating QR…" }
                                            }
                                            div { class: "flex min-w-0 flex-1 flex-col gap-2",
                                                p { class: "text-sm text-muted-foreground",
                                                    "Scan with the MeshCore app to add this headquarters as a contact."
                                                }
                                                div { class: "flex flex-wrap gap-2",
                                                    Button {
                                                        kind: ButtonKind::Ghost,
                                                        onclick: move |_| {
                                                            let escaped = uri_copy
                                                                .replace('\\', "\\\\")
                                                                .replace('`', "\\`")
                                                                .replace('$', "\\$");
                                                            document::eval(&format!(
                                                                "navigator.clipboard.writeText(`{escaped}`).catch(() => {{}})"
                                                            ));
                                                        },
                                                        Icon { name: IconName::Copy, size: 14 }
                                                        "Copy link"
                                                    }
                                                    Button {
                                                        kind: ButtonKind::Ghost,
                                                        disabled: qr_print.is_empty(),
                                                        onclick: move |_| {
                                                            // Prefer a fresh data: URL from disk; metis:// works in the UI
                                                            // but not inside a print iframe.
                                                            let qr_data = state()
                                                                .db
                                                                .qr_contact_row(&key_print)
                                                                .ok()
                                                                .flatten()
                                                                .and_then(|row| row.qr_file)
                                                                .as_deref()
                                                                .and_then(qr_data_url_from_file)
                                                                .or_else(|| {
                                                                    if qr_print.starts_with("data:") {
                                                                        Some(qr_print.clone())
                                                                    } else {
                                                                        None
                                                                    }
                                                                })
                                                                .unwrap_or_default();
                                                            print_contact_sheet(
                                                                &name_print,
                                                                &key_print,
                                                                &uri,
                                                                &qr_data,
                                                            );
                                                        },
                                                        Icon { name: IconName::Inbox, size: 14 }
                                                        "Print"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            div { class: "mb-3",
                                label { class: "mb-1 block text-xs text-muted-foreground", "Share position in advert" }
                                SelectInput {
                                    id: "adv-loc-policy".to_string(),
                                    value: radio.adv_loc_policy.unwrap_or(0).to_string(),
                                    onchange: move |event: FormEvent| {
                                        if let Ok(policy) = event.value().parse::<u8>() {
                                            state().radio.set_adv_loc_policy(policy);
                                            if let Some(key) = share_loc_key.clone() {
                                                state().radio.set_custom_var(key, policy.to_string());
                                            }
                                        }
                                    },
                                    option {
                                        value: "0",
                                        style: "background-color: #101418; color: #f4f4f5;",
                                        "{loc_policy_label(0)}"
                                    }
                                    option {
                                        value: "1",
                                        style: "background-color: #101418; color: #f4f4f5;",
                                        "{loc_policy_label(1)}"
                                    }
                                    option {
                                        value: "2",
                                        style: "background-color: #101418; color: #f4f4f5;",
                                        "{loc_policy_label(2)}"
                                    }
                                }
                            }
                            if gps_key.is_some() || gps_interval_key.is_some() {
                                div { class: "mb-3 rounded-md border border-border bg-background px-3 py-2",
                                    p { class: "mb-2 text-xs uppercase tracking-[0.14em] text-muted-foreground", "GPS" }
                                    if let Some(key) = gps_key.clone() {
                                        label { class: "mb-2 flex cursor-pointer items-center gap-2 text-sm",
                                            input {
                                                r#type: "checkbox",
                                                checked: gps_on,
                                                oninput: move |_| {
                                                    let next = if gps_on { "0" } else { "1" };
                                                    state().radio.set_custom_var(key.clone(), next.into());
                                                },
                                            }
                                            if gps_on { "GPS enabled" } else { "GPS disabled" }
                                        }
                                    }
                                    if let Some(key) = gps_interval_key.clone() {
                                        {
                                            let current = radio.custom_vars.get(&key).cloned().unwrap_or_default();
                                            let draft = if gps_interval_draft().is_empty() {
                                                current.clone()
                                            } else {
                                                gps_interval_draft()
                                            };
                                            rsx! {
                                                div { class: "flex flex-col gap-2 sm:flex-row sm:items-end",
                                                    div { class: "min-w-0 flex-1",
                                                        label { class: "mb-1 block text-xs text-muted-foreground", "GPS interval (seconds)" }
                                                        TextInput {
                                                            value: draft,
                                                            placeholder: current.clone(),
                                                            oninput: move |event: FormEvent| gps_interval_draft.set(event.value()),
                                                        }
                                                    }
                                                    Button {
                                                        kind: ButtonKind::Ghost,
                                                        onclick: move |_| {
                                                            let next = gps_interval_draft().trim().to_string();
                                                            if !next.is_empty() {
                                                                state().radio.set_custom_var(key.clone(), next);
                                                            }
                                                        },
                                                        "Apply interval"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                p { class: "mb-3 text-xs text-muted-foreground",
                                    "GPS toggles appear after Refresh when the companion exposes gps / gps_interval vars."
                                }
                            }
                            p { class: "mb-2 font-mono text-xs text-muted-foreground",
                                if let (Some(lat), Some(lon)) = (radio.hq_lat, radio.hq_lon) {
                                    "Current: {lat:.5}, {lon:.5}"
                                } else {
                                    "Current: not set"
                                }
                            }
                            div { class: "flex flex-col gap-2 sm:flex-row sm:items-end",
                                div { class: "min-w-0 flex-1",
                                    label { class: "mb-1 block text-xs text-muted-foreground", "Latitude" }
                                    TextInput {
                                        value: lat_draft(),
                                        placeholder: radio.hq_lat.map(|v| format!("{v:.6}")).unwrap_or_else(|| "37.774900".into()),
                                        oninput: move |event: FormEvent| lat_draft.set(event.value()),
                                    }
                                }
                                div { class: "min-w-0 flex-1",
                                    label { class: "mb-1 block text-xs text-muted-foreground", "Longitude" }
                                    TextInput {
                                        value: lon_draft(),
                                        placeholder: radio.hq_lon.map(|v| format!("{v:.6}")).unwrap_or_else(|| "-122.419400".into()),
                                        oninput: move |event: FormEvent| lon_draft.set(event.value()),
                                    }
                                }
                                Button {
                                    onclick: move |_| {
                                        let lat = lat_draft().trim().parse::<f64>().ok().or(radio.hq_lat);
                                        let lon = lon_draft().trim().parse::<f64>().ok().or(radio.hq_lon);
                                        if let (Some(lat), Some(lon)) = (lat, lon) {
                                            if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) {
                                                state().radio.set_coords(lat, lon);
                                            }
                                        }
                                    },
                                    Icon { name: IconName::Locate, size: 14 }
                                    "Set location"
                                }
                            }
                            div { class: "mt-3 flex flex-wrap gap-2",
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| state().radio.send_advert(false),
                                    Icon { name: IconName::Radio, size: 14 }
                                    "Send advert"
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| state().radio.send_advert(true),
                                    Icon { name: IconName::Radio, size: 14 }
                                    "Flood advert"
                                }
                            }
                        }
                    }

                    if device_tab() == DeviceTab::Radio {
                        Card { title: "Radio settings".to_string(), icon: Some(IconName::Signal),
                            p { class: "mb-3 text-sm text-muted-foreground",
                                "Frequency, bandwidth, and spreading factor must match your local mesh. Some firmwares need a reboot after Apply."
                            }
                            div { class: "mb-3 flex flex-wrap items-center justify-between gap-2",
                                div { class: "min-w-0 flex-1 rounded-md border border-border bg-background px-3 py-2.5",
                                    p { class: "mb-1.5 text-xs uppercase tracking-[0.14em] text-muted-foreground", "Selected preset" }
                                    p { class: "text-sm font-medium", "{active_preset}" }
                                    p { class: "mt-1 font-mono text-xs text-muted-foreground",
                                        "{current_freq:.3} MHz · SF{current_sf} · BW{current_bw} · CR{current_cr}"
                                    }
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| {
                                        freq_draft.set(format!("{current_freq:.3}"));
                                        bw_draft.set(format!("{}", nearest_bw_option(current_bw)));
                                        sf_draft.set(current_sf.to_string());
                                        cr_draft.set(current_cr.to_string());
                                        repeat_draft.set(radio.client_repeat);
                                        preset_open.set(true);
                                    },
                                    "Choose preset"
                                }
                            }
                            div { class: "radio-fields",
                                div { class: "field-block",
                                    label { "Frequency (MHz)" }
                                    TextInput {
                                        value: freq_draft(),
                                        placeholder: format!("{current_freq:.3}"),
                                        oninput: move |event: FormEvent| freq_draft.set(event.value()),
                                    }
                                }
                                div { class: "field-block",
                                    label { "Bandwidth (kHz)" }
                                    SelectInput {
                                        id: "radio-bw".to_string(),
                                        value: {
                                            let draft = bw_draft().trim().parse::<f64>().ok();
                                            format!("{}", nearest_bw_option(draft.unwrap_or(current_bw)))
                                        },
                                        onchange: move |event: FormEvent| bw_draft.set(event.value()),
                                        for bw in BW_OPTIONS_KHZ.iter().copied() {
                                            option {
                                                key: "{bw}",
                                                value: "{bw}",
                                                style: "background-color: #101418; color: #f4f4f5;",
                                                "{bw}"
                                            }
                                        }
                                    }
                                }
                                div { class: "field-block",
                                    label { "Spreading factor" }
                                    SelectInput {
                                        id: "radio-sf".to_string(),
                                        value: {
                                            let draft = sf_draft().trim().parse::<u8>().ok().unwrap_or(current_sf);
                                            draft.clamp(5, 12).to_string()
                                        },
                                        onchange: move |event: FormEvent| sf_draft.set(event.value()),
                                        for sf in SF_OPTIONS.iter().copied() {
                                            option {
                                                key: "{sf}",
                                                value: "{sf}",
                                                style: "background-color: #101418; color: #f4f4f5;",
                                                "SF{sf}"
                                            }
                                        }
                                    }
                                }
                                div { class: "field-block",
                                    label { "Coding rate" }
                                    SelectInput {
                                        id: "radio-cr".to_string(),
                                        value: {
                                            let draft = cr_draft().trim().parse::<u8>().ok().unwrap_or(current_cr);
                                            draft.clamp(5, 8).to_string()
                                        },
                                        onchange: move |event: FormEvent| cr_draft.set(event.value()),
                                        for cr in CR_OPTIONS.iter().copied() {
                                            option {
                                                key: "{cr}",
                                                value: "{cr}",
                                                style: "background-color: #101418; color: #f4f4f5;",
                                                "CR{cr}"
                                            }
                                        }
                                    }
                                }
                            }
                            label { class: "mb-3 flex cursor-pointer items-center gap-2 text-sm",
                                input {
                                    r#type: "checkbox",
                                    checked: repeat_draft(),
                                    oninput: move |_| repeat_draft.set(!repeat_draft()),
                                }
                                "Enable repeat mode (client off-grid repeat)"
                            }
                            div { class: "mb-3 flex flex-col gap-2 sm:flex-row sm:items-end",
                                div { class: "min-w-0 flex-1",
                                    label { class: "mb-1 block text-xs text-muted-foreground", "Transmit power (dBm)" }
                                    TextInput {
                                        value: tx_draft(),
                                        placeholder: radio.tx_power.map(|v| v.to_string()).unwrap_or_else(|| "22".into()),
                                        oninput: move |event: FormEvent| tx_draft.set(event.value()),
                                    }
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| {
                                        if let Some(power) = tx_draft().trim().parse::<u8>().ok().or(radio.tx_power) {
                                            state().radio.set_tx_power(power.min(max_tx));
                                        }
                                    },
                                    Icon { name: IconName::Signal, size: 14 }
                                    "Apply TX"
                                }
                            }
                            Button {
                                onclick: move |_| {
                                    let freq = freq_draft().trim().parse::<f64>().unwrap_or(current_freq);
                                    let bw = nearest_bw_option(
                                        bw_draft().trim().parse::<f64>().unwrap_or(current_bw),
                                    );
                                    let sf = sf_draft()
                                        .trim()
                                        .parse::<u8>()
                                        .unwrap_or(current_sf)
                                        .clamp(5, 12);
                                    let cr = cr_draft()
                                        .trim()
                                        .parse::<u8>()
                                        .unwrap_or(current_cr)
                                        .clamp(5, 8);
                                    state().radio.set_radio_params(freq, bw, sf, cr, repeat_draft());
                                },
                                Icon { name: IconName::Check, size: 14 }
                                "Apply radio params"
                            }
                        }
                        Card { title: "Battery profile".to_string(), icon: Some(IconName::Signal),
                            p { class: "mb-3 text-sm text-muted-foreground",
                                "Presets TX power, GPS, and advert location. Does not change mesh frequency. Companion `powersaving` is firmware-default on many nRF52 boards and is not a separate toggle here."
                            }
                            div { class: "mb-3 flex flex-col gap-2 sm:flex-row sm:items-end",
                                div { class: "min-w-0 flex-1",
                                    label { class: "mb-1 block text-xs text-muted-foreground", "Profile" }
                                    SelectInput {
                                        id: "battery-profile".to_string(),
                                        value: battery_profile(),
                                        onchange: move |event: FormEvent| {
                                            battery_profile.set(event.value());
                                            battery_note.set(String::new());
                                        },
                                        for profile in [
                                            BatteryProfile::High,
                                            BatteryProfile::Balanced,
                                            BatteryProfile::Saver,
                                        ] {
                                            option {
                                                key: "{profile.value()}",
                                                value: "{profile.value()}",
                                                style: "background-color: #101418; color: #f4f4f5;",
                                                "{profile.label()}"
                                            }
                                        }
                                    }
                                }
                                Button {
                                    disabled: !linked,
                                    onclick: move |_| {
                                        let Some(profile) = BatteryProfile::from_value(&battery_profile()) else {
                                            return;
                                        };
                                        let snap = state().radio.snapshot();
                                        battery_note.set(apply_battery_profile(profile, &snap));
                                    },
                                    Icon { name: IconName::Check, size: 14 }
                                    "Apply profile"
                                }
                            }
                            p { class: "text-xs text-muted-foreground",
                                {
                                    BatteryProfile::from_value(&battery_profile())
                                        .unwrap_or(BatteryProfile::Balanced)
                                        .summary()
                                }
                            }
                            if !battery_note().is_empty() {
                                p { class: "mt-2 text-xs text-sky-200", "{battery_note()}" }
                            }
                        }
                        Card { title: "Reboot companion".to_string(), icon: Some(IconName::Ban),
                            div { class: "flex flex-wrap items-center gap-2",
                                if reboot_armed() {
                                    Button {
                                        kind: ButtonKind::Danger,
                                        onclick: move |_| {
                                            reboot_armed.set(false);
                                            state().radio.reboot();
                                        },
                                        "Confirm reboot"
                                    }
                                    Button {
                                        kind: ButtonKind::Ghost,
                                        onclick: move |_| reboot_armed.set(false),
                                        "Cancel"
                                    }
                                } else {
                                    Button {
                                        kind: ButtonKind::Ghost,
                                        onclick: move |_| reboot_armed.set(true),
                                        Icon { name: IconName::Ban, size: 14 }
                                        "Reboot companion"
                                    }
                                }
                            }
                        }
                    }

                    if device_tab() == DeviceTab::Network {
                        Card { title: "Network".to_string(), icon: Some(IconName::Signal),
                            p { class: "mb-3 text-sm text-muted-foreground",
                                "Flood region scope and path-hash size for this mesh. Region names must match local repeater config exactly (lowercase a-z, 0-9, hyphen)."
                            }
                            p { class: "mb-2 text-xs text-muted-foreground",
                                if let Some(scope) = radio.flood_scope.as_ref() {
                                    "Companion default scope (this session): {scope}"
                                } else {
                                    "Companion default scope (this session): none / unscoped"
                                }
                            }
                            {
                                let _ = regions_list_tick();
                                let known = crate::mesh_regions::load_known_regions(&state().db);
                                let selected = region_select();
                                let select_value = if selected.is_empty()
                                    || known.iter().any(|n| n == &selected)
                                {
                                    selected
                                } else {
                                    String::new()
                                };
                                rsx! {
                                    div { class: "mb-3",
                                        label { class: "mb-1 block text-xs text-muted-foreground", "Default region scope" }
                                        SelectInput {
                                            id: "mesh-region-select".to_string(),
                                            value: select_value,
                                            onchange: move |event: FormEvent| region_select.set(event.value()),
                                            option {
                                                value: "",
                                                style: "background-color:#151a20;color:#f4f4f5;",
                                                "(none / unscoped)"
                                            }
                                            for name in known.iter() {
                                                {
                                                    let name = name.clone();
                                                    rsx! {
                                                        option {
                                                            key: "{name}",
                                                            value: "{name}",
                                                            style: "background-color:#151a20;color:#f4f4f5;",
                                                            "{name}"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    div { class: "mb-3 flex flex-col gap-2 sm:flex-row sm:items-end",
                                        div { class: "min-w-0 flex-1",
                                            label { class: "mb-1 block text-xs text-muted-foreground", "Add region manually" }
                                            TextInput {
                                                value: region_manual(),
                                                placeholder: "e.g. socal-west",
                                                oninput: move |event: FormEvent| region_manual.set(event.value()),
                                            }
                                        }
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: move |_| {
                                                match crate::mesh_regions::add_known_region(
                                                    &state().db,
                                                    &region_manual(),
                                                ) {
                                                    Ok(name) => {
                                                        region_select.set(name.clone());
                                                        region_manual.set(String::new());
                                                        regions_list_tick.with_mut(|n| *n = n.wrapping_add(1));
                                                        region_note.set(format!("Added region {name}."));
                                                        status_note(format!("Region added: {name}"));
                                                    }
                                                    Err(err) => {
                                                        region_note.set(err.clone());
                                                        crate::toast::push(crate::toast::ToastKind::Error, err);
                                                    }
                                                }
                                            },
                                            "Add"
                                        }
                                    }
                                    div { class: "mb-3 flex flex-wrap gap-2",
                                        if region_scan_busy() {
                                            Button {
                                                kind: ButtonKind::Danger,
                                                onclick: move |_| {
                                                    state().radio.cancel_mesh_scan();
                                                    region_note.set("Stopping region scan…".into());
                                                },
                                                "Stop scan"
                                            }
                                        } else {
                                            Button {
                                                kind: ButtonKind::Ghost,
                                                disabled: !linked || node_discover_busy(),
                                                onclick: move |_| {
                                                    if region_scan_busy() {
                                                        return;
                                                    }
                                                    region_scan_busy.set(true);
                                                    region_note.set("Scanning nearby repeaters/rooms…".into());
                                                    let rx = state().radio.discover_regions();
                                                    spawn(async move {
                                                        let result = rx.await.unwrap_or_else(|_| {
                                                            Err("Region scan cancelled.".into())
                                                        });
                                                        region_scan_busy.set(false);
                                                        match result {
                                                            Ok(names) => {
                                                                let n = names.len();
                                                                match crate::mesh_regions::merge_known_regions(
                                                                    &state().db,
                                                                    &names,
                                                                ) {
                                                                    Ok(_) => {
                                                                        let _ = state().db.audit(
                                                                            "regions_scan",
                                                                            "",
                                                                            &format!("{n} regions"),
                                                                        );
                                                                        regions_list_tick.with_mut(|t| {
                                                                            *t = t.wrapping_add(1)
                                                                        });
                                                                        region_note.set(format!(
                                                                            "Scan found {n} region{}.",
                                                                            if n == 1 { "" } else { "s" }
                                                                        ));
                                                                        status_note(format!(
                                                                            "Region scan: {n} found"
                                                                        ));
                                                                    }
                                                                    Err(err) => {
                                                                        region_note.set(err.clone());
                                                                        crate::toast::push(
                                                                            crate::toast::ToastKind::Error,
                                                                            err,
                                                                        );
                                                                    }
                                                                }
                                                            }
                                                            Err(err) => {
                                                                region_note.set(err.clone());
                                                                crate::toast::push(
                                                                    crate::toast::ToastKind::Error,
                                                                    err,
                                                                );
                                                            }
                                                        }
                                                    });
                                                },
                                                Icon { name: IconName::Scan, size: 14 }
                                                "Scan regions"
                                            }
                                        }
                                        Button {
                                            disabled: !linked,
                                            onclick: move |_| {
                                                let raw = region_select();
                                                let scope = if raw.trim().is_empty() {
                                                    None
                                                } else {
                                                    Some(raw.trim().to_string())
                                                };
                                                match crate::mesh_regions::save_active_region(
                                                    &state().db,
                                                    scope.as_deref(),
                                                ) {
                                                    Ok(()) => {
                                                        state()
                                                            .radio
                                                            .set_default_flood_scope(scope.clone());
                                                        let _ = state().db.audit(
                                                            "region_apply",
                                                            "",
                                                            scope.as_deref().unwrap_or(""),
                                                        );
                                                        let msg = match scope.as_deref() {
                                                            Some(name) => format!("Applied scope {name}"),
                                                            None => "Cleared flood scope".into(),
                                                        };
                                                        region_note.set(msg.clone());
                                                        status_note(msg);
                                                    }
                                                    Err(err) => {
                                                        region_note.set(err.clone());
                                                        crate::toast::push(
                                                            crate::toast::ToastKind::Error,
                                                            err,
                                                        );
                                                    }
                                                }
                                            },
                                            Icon { name: IconName::Check, size: 14 }
                                            "Apply scope"
                                        }
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            disabled: !linked,
                                            onclick: move |_| {
                                                region_select.set(String::new());
                                                let _ = crate::mesh_regions::save_active_region(
                                                    &state().db,
                                                    None,
                                                );
                                                state().radio.set_default_flood_scope(None);
                                                let _ = state().db.audit("region_apply", "", "");
                                                region_note.set("Cleared flood scope.".into());
                                                status_note("Flood scope cleared");
                                            },
                                            "Clear"
                                        }
                                    }
                                    if !region_note().is_empty() {
                                        p { class: "mb-3 text-sm text-muted-foreground", "{region_note}" }
                                    }
                                }
                            }
                            div { class: "mb-4 mt-4 border-t border-border pt-4",
                                p { class: "mb-1 text-xs uppercase tracking-[0.14em] text-muted-foreground",
                                    "Discover nearby"
                                }
                                p { class: "mb-2 text-xs text-muted-foreground",
                                    "Zero-hop scan (direct radio range). Repeaters/rooms or sensors that answer are added to the companion contact table and HQ roster."
                                }
                                div { class: "mb-2 flex flex-wrap gap-2",
                                    if node_discover_busy() {
                                        Button {
                                            kind: ButtonKind::Danger,
                                            onclick: move |_| {
                                                state().radio.cancel_mesh_scan();
                                                node_discover_note.set("Stopping discover…".into());
                                            },
                                            "Stop discover"
                                        }
                                    } else {
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            disabled: !linked || region_scan_busy(),
                                            onclick: move |_| {
                                                if node_discover_busy() {
                                                    return;
                                                }
                                                node_discover_busy.set(true);
                                                node_discover_note.set("Discovering repeaters…".into());
                                                let rx = state().radio.discover_nodes(
                                                    crate::radio::DISCOVER_FILTER_REPEATERS,
                                                );
                                                spawn(async move {
                                                    let result = rx.await.unwrap_or_else(|_| {
                                                        Err("Discover cancelled.".into())
                                                    });
                                                    node_discover_busy.set(false);
                                                    match result {
                                                        Ok(summary) => {
                                                            let msg = format!(
                                                                "Repeaters: {} returned, {} new.",
                                                                summary.returned, summary.added
                                                            );
                                                            node_discover_note.set(msg.clone());
                                                            status_note(msg);
                                                        }
                                                        Err(err) => {
                                                            node_discover_note.set(err.clone());
                                                            crate::toast::push(
                                                                crate::toast::ToastKind::Error,
                                                                err,
                                                            );
                                                        }
                                                    }
                                                });
                                            },
                                            Icon { name: IconName::Scan, size: 14 }
                                            "Discover repeaters"
                                        }
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            disabled: !linked || region_scan_busy(),
                                            onclick: move |_| {
                                                if node_discover_busy() {
                                                    return;
                                                }
                                                node_discover_busy.set(true);
                                                node_discover_note.set("Discovering sensors…".into());
                                                let rx = state().radio.discover_nodes(
                                                    crate::radio::DISCOVER_FILTER_SENSORS,
                                                );
                                                spawn(async move {
                                                    let result = rx.await.unwrap_or_else(|_| {
                                                        Err("Discover cancelled.".into())
                                                    });
                                                    node_discover_busy.set(false);
                                                    match result {
                                                        Ok(summary) => {
                                                            let msg = format!(
                                                                "Sensors: {} returned, {} new.",
                                                                summary.returned, summary.added
                                                            );
                                                            node_discover_note.set(msg.clone());
                                                            status_note(msg);
                                                        }
                                                        Err(err) => {
                                                            node_discover_note.set(err.clone());
                                                            crate::toast::push(
                                                                crate::toast::ToastKind::Error,
                                                                err,
                                                            );
                                                        }
                                                    }
                                                });
                                            },
                                            Icon { name: IconName::Scan, size: 14 }
                                            "Discover sensors"
                                        }
                                    }
                                }
                                if !node_discover_note().is_empty() {
                                    p { class: "mb-2 text-sm text-muted-foreground", "{node_discover_note}" }
                                }
                            }
                            div { class: "mt-2",
                                p { class: "mb-1 text-xs uppercase tracking-[0.14em] text-muted-foreground",
                                    "Default path hash size"
                                }
                                p { class: "mb-2 text-xs text-muted-foreground",
                                    if let Some(mode) = path_hash_mode {
                                        "Current: {mode}-mode ({mode + 1}-byte). SoCal/Canada-style presets set 3-byte automatically."
                                    } else {
                                        "Current: unknown until set. Companion opcode 61 + custom var when present."
                                    }
                                }
                                if let Some(key) = path_hash_key.clone() {
                                    p { class: "mb-2 font-mono text-xs text-muted-foreground",
                                        "{key} = {radio.custom_vars.get(&key).cloned().unwrap_or_default()}"
                                    }
                                }
                                div { class: "flex flex-wrap gap-2",
                                    Button {
                                        kind: if path_hash_mode == Some(0) { ButtonKind::Primary } else { ButtonKind::Ghost },
                                        onclick: move |_| apply_path_hash_mode_ui(0),
                                        "1-byte"
                                    }
                                    Button {
                                        kind: if path_hash_mode == Some(1) { ButtonKind::Primary } else { ButtonKind::Ghost },
                                        onclick: move |_| apply_path_hash_mode_ui(1),
                                        "2-byte"
                                    }
                                    Button {
                                        kind: if path_hash_mode == Some(2) { ButtonKind::Primary } else { ButtonKind::Ghost },
                                        onclick: move |_| apply_path_hash_mode_ui(2),
                                        "3-byte"
                                    }
                                }
                            }
                        }
                    }

                    if device_tab() == DeviceTab::Advanced {
                        Card { title: "Custom vars".to_string(), icon: Some(IconName::Settings),
                            div { class: "mb-3 flex items-center justify-between gap-2",
                                p { class: "text-sm text-muted-foreground",
                                    "Raw firmware key/value settings (GPS, display sleep, and more when present)."
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| state().radio.refresh_device_settings(),
                                    Icon { name: IconName::Scan, size: 14 }
                                    "Refresh"
                                }
                            }
                            if radio.custom_vars.is_empty() {
                                p { class: "text-sm text-muted-foreground",
                                    "No custom vars reported yet."
                                }
                            } else {
                                div { class: "flex flex-col gap-2",
                                    for (key, value) in radio.custom_vars.iter() {
                                        {
                                            let key = key.clone();
                                            let value = value.clone();
                                            let label = custom_var_label(&key);
                                            let boolish = is_boolish_var(&value);
                                            let draft = var_drafts()
                                                .get(&key)
                                                .cloned()
                                                .unwrap_or_else(|| value.clone());
                                            rsx! {
                                                div {
                                                    key: "{key}",
                                                    class: "flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-background px-3 py-2",
                                                    div { class: "min-w-0",
                                                        p { class: "text-sm font-medium", "{label}" }
                                                        p { class: "font-mono text-[0.65rem] text-muted-foreground", "{key}" }
                                                    }
                                                    if let Some(on) = boolish {
                                                        Button {
                                                            kind: ButtonKind::Ghost,
                                                            onclick: {
                                                                let key = key.clone();
                                                                let next = flip_boolish_var(&value);
                                                                move |_| state().radio.set_custom_var(key.clone(), next.clone())
                                                            },
                                                            if on { "On · turn off" } else { "Off · turn on" }
                                                        }
                                                    } else {
                                                        div { class: "flex min-w-0 flex-1 items-center gap-2 sm:max-w-xs",
                                                            TextInput {
                                                                value: draft.clone(),
                                                                placeholder: value.clone(),
                                                                oninput: {
                                                                    let key = key.clone();
                                                                    move |event: FormEvent| {
                                                                        let mut map = var_drafts();
                                                                        map.insert(key.clone(), event.value());
                                                                        var_drafts.set(map);
                                                                    }
                                                                },
                                                            }
                                                            Button {
                                                                kind: ButtonKind::Ghost,
                                                                onclick: {
                                                                    let key = key.clone();
                                                                    let fallback = value.clone();
                                                                    move |_| {
                                                                        let next = var_drafts()
                                                                            .get(&key)
                                                                            .cloned()
                                                                            .unwrap_or(fallback.clone());
                                                                        state().radio.set_custom_var(key.clone(), next);
                                                                    }
                                                                },
                                                                "Set"
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
                    } // device-settings-panel
                }

                Dialog {
                    open: preset_open(),
                    title: "Select radio settings".to_string(),
                    on_close: move |_| preset_open.set(false),
                    p { class: "mb-3 text-sm text-muted-foreground",
                        "Official MeshCore community presets (API + offline cache). USA and USA - Southern California are marked Recommended for North America."
                    }
                    div { class: "mb-3",
                        TextInput {
                            value: preset_query(),
                            placeholder: "Search",
                            oninput: move |event: FormEvent| preset_query.set(event.value()),
                        }
                    }
                    div { class: "preset-list",
                        for preset in filtered_presets.iter() {
                            {
                                let preset = preset.clone();
                                let row_class = if preset.recommended {
                                    "preset-row preset-row-recommended"
                                } else {
                                    "preset-row"
                                };
                                rsx! {
                                    button {
                                        key: "{preset.name}",
                                        class: "{row_class}",
                                        onclick: move |_| {
                                            freq_draft.set(format!("{:.3}", preset.freq_mhz));
                                            bw_draft.set(format!("{}", nearest_bw_option(preset.bw_khz)));
                                            sf_draft.set(preset.sf.to_string());
                                            cr_draft.set(preset.cr.to_string());
                                            if let Some(mode) = preset.path_hash_mode() {
                                                queue_path_hash_mode(mode);
                                            }
                                            state().radio.set_radio_params(
                                                preset.freq_mhz,
                                                preset.bw_khz,
                                                preset.sf,
                                                preset.cr,
                                                repeat_draft(),
                                            );
                                            preset_open.set(false);
                                            preset_query.set(String::new());
                                        },
                                        span { class: "text-sm font-medium",
                                            "{preset.name}"
                                            if preset.recommended {
                                                " · Recommended"
                                            }
                                        }
                                        span { class: "font-mono text-xs text-muted-foreground", "{preset.summary()}" }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if settings_tab() == SettingsTab::Flasher {
            {
                let manage_open = flasher_manage_open();
                // Rebuild device/catalog UI only on flasher_tick (Sync / integrity / import),
                // not every 350ms — load_catalog + index + asset resolve was freezing the tab.
                let flasher_bump = flasher_tick();
                let _flasher_refresh = flasher_bump;
                let sync = crate::firmware::sync_status();
                let flash = crate::firmware::flash_status();
                let sync_busy = sync.running;
                let flash_busy = flash.running;
                // Light poll for sync/flash status text without reloading the catalog.
                let _sync_poll = if manage_open {
                    0u32
                } else {
                    log_tick()
                };
                let last_sync_label = if sync.last_ok_at > 0 {
                    crate::timefmt::format_stamp(&state().db, sync.last_ok_at)
                } else {
                    String::new()
                };
                let selection = crate::firmware::load_selection();
                let n_selected = selection.entries.len();
                let disk_label = crate::firmware::format_bytes(sync.bytes_on_disk);
                let full_set_label = crate::firmware::estimate_full_set_label();
                let readiness = if manage_open {
                    None
                } else {
                    flasher_readiness()
                };
                // While Manage is open, skip index/catalog/USB work — only the cached offer list.
                let on_disk_keys = flasher_manage_on_disk();
                let unavailable_keys = flasher_manage_unavailable();
                let manage_groups: Vec<(i32, String, String, Vec<crate::firmware::FirmwareOffer>)> =
                    if manage_open {
                        let manage_query = flasher_manage_query();
                        let mut by_device: std::collections::BTreeMap<
                            String,
                            (i32, String, Vec<crate::firmware::FirmwareOffer>),
                        > = std::collections::BTreeMap::new();
                        for offer in flasher_manage_offers() {
                            let Some(score) = fuzzy_score(
                                &manage_query,
                                &[
                                    &offer.maker_label,
                                    &offer.device,
                                    &offer.role_label,
                                    &offer.role_pill,
                                ],
                            ) else {
                                continue;
                            };
                            let entry = by_device.entry(offer.device.clone()).or_insert_with(|| {
                                (score, offer.maker_label.clone(), Vec::new())
                            });
                            entry.0 = entry.0.max(score);
                            entry.2.push(offer);
                        }
                        let mut groups: Vec<(i32, String, String, Vec<crate::firmware::FirmwareOffer>)> =
                            by_device
                                .into_iter()
                                .map(|(device, (score, maker, mut roles))| {
                                    roles.sort_by(|a, b| a.role.cmp(&b.role));
                                    (score, device, maker, roles)
                                })
                                .collect();
                        groups.sort_by(|a, b| {
                            b.0.cmp(&a.0)
                                .then(a.2.to_ascii_lowercase().cmp(&b.2.to_ascii_lowercase()))
                                .then(a.1.to_ascii_lowercase().cmp(&b.1.to_ascii_lowercase()))
                        });
                        groups
                    } else {
                        Vec::new()
                    };
                let (
                    filtered,
                    selected_device,
                    selected_role,
                    version_options,
                    ports,
                    asset_ready,
                    catalog_missing,
                ) = if manage_open {
                    (
                        Vec::new(),
                        None,
                        None,
                        Vec::new(),
                        Vec::new(),
                        None,
                        flasher_manage_offers().is_empty(),
                    )
                } else {
                    let cache_key = format!(
                        "{flasher_bump}|{}|{}|{}|{}|{}|{}|{}|{}",
                        flasher_query(),
                        flasher_device(),
                        flasher_role(),
                        flasher_tag(),
                        flasher_erase(),
                        sync.asset_count,
                        sync.last_ok_at,
                        n_selected,
                    );
                    let panel = load_flasher_panel_cache(
                        cache_key,
                        &selection,
                        &flasher_device(),
                        &flasher_role(),
                        &flasher_tag(),
                        flasher_erase(),
                        &flasher_query(),
                    );
                    if let Some(dev) = panel.selected_device.as_ref() {
                        if flasher_device() != dev.name {
                            flasher_device.set(dev.name.clone());
                        }
                    }
                    if let Some(role) = panel.selected_role.as_ref() {
                        if flasher_role() != role.role {
                            flasher_role.set(role.role.clone());
                        }
                    }
                    if !panel.version_options.is_empty()
                        && panel
                            .version_options
                            .iter()
                            .all(|(tag, _)| tag != &flasher_tag())
                    {
                        flasher_tag.set(panel.version_options[0].0.clone());
                    }
                    if flasher_port().is_empty() {
                        if let Some(port) = panel.ports.first() {
                            flasher_port.set(port.path.clone());
                        }
                    }
                    (
                        panel.filtered,
                        panel.selected_device,
                        panel.selected_role,
                        panel.version_options,
                        panel.ports,
                        panel.asset_ready,
                        panel.catalog_missing,
                    )
                };
                rsx! {
                    Card { title: "Firmware library".to_string(), icon: Some(IconName::Radio),
                        div { class: "flasher-lib",
                        p { class: "flasher-lib__intro",
                            "Keep MeshCore community firmware offline for USB flashing. Choose boards in Manage, sync while online; flash is blocked if a checksum fails."
                        }

                        // --- Outage status ---
                        if let Some(ready) = readiness.as_ref() {
                            p {
                                class: if ready.ready_for_outage {
                                    "flasher-lib__banner flasher-lib__banner--ready"
                                } else {
                                    "flasher-lib__banner flasher-lib__banner--warn"
                                },
                                "{ready.summary}"
                            }
                        } else if sync_busy {
                            p { class: "flasher-lib__meta", "Syncing · {sync.detail}" }
                        } else if !sync.last_error.is_empty() {
                            p { class: "flasher-lib__meta flasher-lib__meta-warn", "Last error · {sync.last_error}" }
                        }

                        // --- Compact metrics ---
                        {
                            let (sel, on_disk, verified, missing, unpublished, bad, github, deep) =
                                if let Some(ready) = readiness.as_ref() {
                                    (
                                        ready.selected,
                                        ready.on_disk_assets,
                                        ready.verified_ok,
                                        ready.missing_selected,
                                        ready.unpublished_selected,
                                        ready.corrupt_or_incomplete,
                                        ready.github_verified,
                                        ready.deep_checked,
                                    )
                                } else {
                                    (n_selected, sync.asset_count as usize, 0, 0, 0, 0, 0, false)
                                };
                            let verified_label = if on_disk > 0 {
                                format!("{verified}/{on_disk}")
                            } else {
                                "—".into()
                            };
                            rsx! {
                                div { class: "flasher-lib__metrics",
                                    div { class: "flasher-lib__metric",
                                        p { class: "flasher-lib__metric-label", "Selected" }
                                        p { class: "flasher-lib__metric-value", "{sel}" }
                                    }
                                    div { class: "flasher-lib__metric",
                                        p { class: "flasher-lib__metric-label", "On disk" }
                                        p { class: "flasher-lib__metric-value", "{on_disk}" }
                                    }
                                    div { class: "flasher-lib__metric",
                                        p { class: "flasher-lib__metric-label", "Size" }
                                        p { class: "flasher-lib__metric-value", "{disk_label}" }
                                    }
                                    div { class: "flasher-lib__metric",
                                        p { class: "flasher-lib__metric-label",
                                            if deep { "Checksum OK" } else { "Verified" }
                                        }
                                        p { class: "flasher-lib__metric-value", "{verified_label}" }
                                    }
                                }
                                if missing > 0 || bad > 0 || unpublished > 0 || github > 0 {
                                    p { class: "flasher-lib__meta",
                                        if missing > 0 {
                                            span { "Missing {missing}" }
                                        }
                                        if bad > 0 {
                                            if missing > 0 { span { " · " } }
                                            span { class: "flasher-lib__meta-warn", "Bad {bad}" }
                                        }
                                        if unpublished > 0 {
                                            if missing > 0 || bad > 0 { span { " · " } }
                                            span { "Unpublished {unpublished} (OK)" }
                                        }
                                        if github > 0 {
                                            if missing > 0 || bad > 0 || unpublished > 0 { span { " · " } }
                                            span { "{github} GitHub-verified" }
                                        }
                                    }
                                }
                            }
                        }
                        if !last_sync_label.is_empty() {
                            p { class: "flasher-lib__meta",
                                "Last successful sync · {last_sync_label}"
                            }
                        }

                        // --- Library actions ---
                        div { class: "flasher-lib__section",
                            p { class: "flasher-lib__section-title", "Library" }
                            div { class: "flasher-lib__actions",
                                Button {
                                    disabled: sync_busy,
                                    onclick: move |_| {
                                        crate::firmware::start_sync(state().db.clone());
                                        flasher_tick.with_mut(|n| *n = n.wrapping_add(1));
                                    },
                                    Icon { name: IconName::Scan, size: 14 }
                                    if sync_busy { "Syncing…" } else { "Sync now" }
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| {
                                        let catalog = crate::firmware::load_catalog().ok();
                                        let offers = catalog
                                            .as_ref()
                                            .map(crate::firmware::community_offers)
                                            .unwrap_or_default();
                                        // Open immediately with a cheap draft; disk hints load in background.
                                        // Do not let background work rewrite draft / Download-all (race).
                                        let had_selection = crate::firmware::selection_file_exists();
                                        let sel = if had_selection {
                                            crate::firmware::load_selection()
                                        } else {
                                            crate::firmware::FirmwareSelection::default()
                                        };
                                        let mut draft = std::collections::BTreeMap::new();
                                        for entry in sel.entries.iter() {
                                            draft.insert(entry.key.clone(), entry.clone());
                                        }
                                        let all_selected = sel.download_all
                                            || crate::firmware::selection_covers_all_offers(
                                                &sel,
                                                &offers,
                                            );
                                        flasher_manage_offers.set(offers.clone());
                                        flasher_manage_on_disk.set(std::collections::BTreeSet::new());
                                        flasher_manage_unavailable
                                            .set(crate::firmware::load_unavailable_keys());
                                        flasher_manage_family_tags
                                            .set(std::collections::BTreeMap::new());
                                        flasher_manage_draft.set(draft);
                                        flasher_manage_query.set(String::new());
                                        flasher_manage_expanded
                                            .set(std::collections::BTreeSet::new());
                                        flasher_download_all.set(all_selected);
                                        flasher_manage_open.set(true);
                                        spawn(async move {
                                            let offers_bg = offers;
                                            let result = tokio::task::spawn_blocking(move || {
                                                let index = crate::firmware::load_index();
                                                let seeded = if had_selection {
                                                    None
                                                } else {
                                                    Some(crate::firmware::load_or_seed_selection(
                                                        &offers_bg,
                                                        &index,
                                                    ))
                                                };
                                                let on_disk = crate::firmware::local_offer_keys(
                                                    &offers_bg,
                                                    &index,
                                                );
                                                let family_tags =
                                                    crate::firmware::family_tags_from_index(
                                                        &index,
                                                    );
                                                let unavailable =
                                                    crate::firmware::load_unavailable_keys();
                                                (seeded, on_disk, family_tags, unavailable)
                                            })
                                            .await;
                                            match result {
                                                Ok((seeded, on_disk, family_tags, unavailable)) => {
                                                    if let Some(seeded) = seeded {
                                                        if !seeded.entries.is_empty()
                                                            && flasher_manage_draft().is_empty()
                                                        {
                                                            let mut draft_map =
                                                                std::collections::BTreeMap::new();
                                                            for entry in seeded.entries {
                                                                draft_map.insert(
                                                                    entry.key.clone(),
                                                                    entry,
                                                                );
                                                            }
                                                            let all_selected = crate::firmware::selection_covers_all_offers(
                                                                &crate::firmware::FirmwareSelection {
                                                                    download_all: false,
                                                                    entries: draft_map
                                                                        .values()
                                                                        .cloned()
                                                                        .collect(),
                                                                    updated_at: 0,
                                                                },
                                                                &flasher_manage_offers(),
                                                            );
                                                            flasher_manage_draft.set(draft_map);
                                                            flasher_download_all.set(all_selected);
                                                        }
                                                    }
                                                    flasher_manage_on_disk.set(on_disk);
                                                    flasher_manage_unavailable.set(unavailable);
                                                    flasher_manage_family_tags.set(family_tags);
                                                }
                                                Err(err) => {
                                                    hq_log!(
                                                        warn,
                                                        "firmware manage preload: {err}"
                                                    );
                                                }
                                            }
                                        });
                                    },
                                    "Manage firmware…"
                                }
                            }
                        }

                        // --- USB pack ---
                        div { class: "flasher-lib__section",
                            p { class: "flasher-lib__section-title", "USB pack" }
                            div { class: "flasher-lib__actions",
                                Button {
                                    kind: ButtonKind::Ghost,
                                    disabled: sync_busy,
                                    onclick: move |_| {
                                        flasher_pack_note.set("Checking integrity…".into());
                                        spawn(async move {
                                            let report = tokio::task::spawn_blocking(
                                                crate::firmware::readiness_report,
                                            )
                                            .await;
                                            match report {
                                                Ok(r) => {
                                                    let kind = if r.ready_for_outage {
                                                        crate::toast::ToastKind::Success
                                                    } else {
                                                        crate::toast::ToastKind::Warn
                                                    };
                                                    crate::toast::push(kind, r.summary.clone());
                                                    flasher_readiness.set(Some(r));
                                                    flasher_pack_note.set(String::new());
                                                }
                                                Err(err) => {
                                                    let msg = format!(
                                                        "Integrity check failed: {err}"
                                                    );
                                                    flasher_pack_note.set(String::new());
                                                    crate::toast::push(
                                                        crate::toast::ToastKind::Error,
                                                        msg,
                                                    );
                                                }
                                            }
                                        });
                                    },
                                    "Check integrity"
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    disabled: sync_busy,
                                    onclick: move |_| {
                                        let dest = match crate::firmware::pick_export_locker_pack_path()
                                        {
                                            Ok(path) => path,
                                            Err(_) => return, // cancelled
                                        };
                                        flasher_pack_note.set("Exporting zip pack…".into());
                                        spawn(async move {
                                            let result = tokio::task::spawn_blocking(move || {
                                                crate::firmware::export_locker_pack_to(&dest)
                                            })
                                            .await;
                                            match result {
                                                Ok(Ok(path)) => {
                                                    flasher_pack_note.set(String::new());
                                                    crate::toast::push(
                                                        crate::toast::ToastKind::Success,
                                                        format!(
                                                            "Firmware pack exported to {}",
                                                            path.display()
                                                        ),
                                                    );
                                                }
                                                Ok(Err(err)) => {
                                                    flasher_pack_note.set(String::new());
                                                    crate::toast::push(
                                                        crate::toast::ToastKind::Error,
                                                        err,
                                                    );
                                                }
                                                Err(err) => {
                                                    flasher_pack_note.set(String::new());
                                                    crate::toast::push(
                                                        crate::toast::ToastKind::Error,
                                                        format!("Export failed: {err}"),
                                                    );
                                                }
                                            }
                                        });
                                    },
                                    "Export pack"
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    disabled: sync_busy,
                                    onclick: move |_| {
                                        let zip_path = match crate::firmware::pick_import_locker_pack_path()
                                        {
                                            Ok(path) => path,
                                            Err(_) => return,
                                        };
                                        flasher_pack_note.set("Importing zip pack…".into());
                                        spawn(async move {
                                            let result = tokio::task::spawn_blocking(move || {
                                                crate::firmware::import_locker_pack_from(
                                                    &zip_path,
                                                )
                                            })
                                            .await;
                                            match result {
                                                Ok(Ok(_)) => {
                                                    flasher_pack_note.set(String::new());
                                                    crate::toast::push(
                                                        crate::toast::ToastKind::Success,
                                                        "Firmware pack imported. Run Check integrity.",
                                                    );
                                                    flasher_tick.with_mut(|n| *n = n.wrapping_add(1));
                                                }
                                                Ok(Err(err)) => {
                                                    flasher_pack_note.set(String::new());
                                                    crate::toast::push(
                                                        crate::toast::ToastKind::Error,
                                                        err,
                                                    );
                                                }
                                                Err(err) => {
                                                    flasher_pack_note.set(String::new());
                                                    crate::toast::push(
                                                        crate::toast::ToastKind::Error,
                                                        format!("Import failed: {err}"),
                                                    );
                                                }
                                            }
                                        });
                                    },
                                    "Import pack"
                                }
                            }
                            if !flasher_pack_note().is_empty() {
                                p { class: "flasher-lib__note", "{flasher_pack_note}" }
                            }
                        }

                        // --- On-disk folder ---
                        {
                            let folder = crate::firmware::firmware_root_display();
                            rsx! {
                                div { class: "flasher-lib__panel",
                                    p { class: "flasher-lib__section-title", "On this computer" }
                                    p { class: "flasher-lib__path", "{folder}" }
                                    div { class: "flasher-lib__actions",
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: {
                                                let folder = folder.clone();
                                                move |_| {
                                                    let escaped = folder
                                                        .replace('\\', "\\\\")
                                                        .replace('`', "\\`")
                                                        .replace('$', "\\$");
                                                    let _ = document::eval(
                                                        &format!(
                                                            "navigator.clipboard.writeText(`{escaped}`).catch(() => {{}})"
                                                        ),
                                                    );
                                                    crate::toast::push(
                                                        crate::toast::ToastKind::Success,
                                                        "Firmware path copied",
                                                    );
                                                }
                                            },
                                            Icon { name: IconName::Copy, size: 14 }
                                            "Copy path"
                                        }
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: move |_| {
                                                match crate::firmware::open_firmware_folder() {
                                                    Ok(()) => status_note("Opening firmware folder…"),
                                                    Err(err) => {
                                                        hq_log!(warn, "open firmware folder: {err}");
                                                        status_note(format!("Open folder failed: {err}"));
                                                    }
                                                }
                                            },
                                            "Open folder"
                                        }
                                    }
                                }
                            }
                        }

                        // --- Background sync ---
                        div { class: "flasher-lib__panel",
                            p { class: "flasher-lib__section-title", "Background sync" }
                            div { class: "flasher-lib__sync-row",
                                label { class: "flasher-lib__sync-toggle",
                                    input {
                                        r#type: "checkbox",
                                        checked: flasher_sync_enabled(),
                                        oninput: move |_| {
                                            let next = !flasher_sync_enabled();
                                            flasher_sync_enabled.set(next);
                                            crate::services::set_setting_bool(
                                                &state().db,
                                                "firmware_sync_enabled",
                                                next,
                                            );
                                        }
                                    }
                                    "When online"
                                }
                                div { class: "flasher-lib__sync-interval",
                                    label {
                                        r#for: "firmware-sync-interval",
                                        "Check every"
                                    }
                                    SelectInput {
                                        id: "firmware-sync-interval".to_string(),
                                        value: flasher_sync_interval(),
                                        onchange: move |event: FormEvent| {
                                            let raw = event.value();
                                            let hours = crate::firmware::normalize_sync_interval_hours(
                                                raw.parse::<u64>().unwrap_or(12),
                                            );
                                            flasher_sync_interval.set(hours.to_string());
                                            let _ = state().db.set_setting(
                                                "firmware_sync_interval_hours",
                                                &hours.to_string(),
                                            );
                                        },
                                        option {
                                            value: "12",
                                            style: "background-color: #101418; color: #f4f4f5;",
                                            "12 hours"
                                        }
                                        option {
                                            value: "24",
                                            style: "background-color: #101418; color: #f4f4f5;",
                                            "1 day"
                                        }
                                        option {
                                            value: "720",
                                            style: "background-color: #101418; color: #f4f4f5;",
                                            "30 days"
                                        }
                                    }
                                }
                            }
                        }
                        } // .flasher-lib
                    }
                    SlideDownPanel {
                        open: flasher_manage_open(),
                        title: "Manage firmware".to_string(),
                        subtitle: "Choose boards and roles to keep offline. Unchecked packages stop updating and are pruned on the next successful sync.".to_string(),
                        icon: Some(IconName::Radio),
                        wide: true,
                        on_close: move |_| flasher_manage_open.set(false),
                        footer: rsx! {
                            Button {
                                disabled: sync_busy,
                                onclick: move |_| {
                                    let draft = flasher_manage_draft();
                                    let offers = flasher_manage_offers();
                                    let entries: Vec<_> = draft.into_values().collect();
                                    let mut sel = crate::firmware::FirmwareSelection {
                                        updated_at: 0,
                                        download_all: flasher_download_all(),
                                        entries,
                                    };
                                    if crate::firmware::selection_covers_all_offers(&sel, &offers) {
                                        sel.download_all = true;
                                    }
                                    if let Err(err) = crate::firmware::save_selection(&sel) {
                                        hq_log!(warn, "save firmware selection: {err}");
                                        return;
                                    }
                                    flasher_manage_open.set(false);
                                    crate::firmware::start_sync(state().db.clone());
                                    flasher_tick.with_mut(|n| *n = n.wrapping_add(1));
                                },
                                Icon { name: IconName::Scan, size: 14 }
                                "Save & sync"
                            }
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| flasher_manage_open.set(false),
                                "Close"
                            }
                        },
                        div { class: "mb-3",
                            TextInput {
                                value: flasher_manage_query(),
                                placeholder: "Search maker, model, or role…",
                                oninput: move |event: FormEvent| {
                                    let q = event.value();
                                    flasher_manage_query.set(q.clone());
                                    // Auto-expand small search results; keep full list collapsed.
                                    if q.trim().len() >= 2 {
                                        let offers = flasher_manage_offers();
                                        let mut devices = std::collections::BTreeSet::new();
                                        for offer in offers.iter() {
                                            if fuzzy_score(
                                                &q,
                                                &[
                                                    &offer.maker_label,
                                                    &offer.device,
                                                    &offer.role_label,
                                                    &offer.role_pill,
                                                ],
                                            )
                                            .is_some()
                                            {
                                                devices.insert(offer.device.clone());
                                            }
                                        }
                                        if devices.len() <= 8 {
                                            flasher_manage_expanded.set(devices);
                                        }
                                    }
                                },
                            }
                        }
                        label { class: "mb-2 flex items-center gap-2 text-sm",
                            input {
                                r#type: "checkbox",
                                checked: flasher_download_all(),
                                oninput: move |_| {
                                    let next = !flasher_download_all();
                                    flasher_download_all.set(next);
                                    if next {
                                        let offers = flasher_manage_offers();
                                        let mut draft = flasher_manage_draft();
                                        for offer in offers {
                                            draft.insert(
                                                offer.key.clone(),
                                                crate::firmware::SelectionEntry {
                                                    key: offer.key.clone(),
                                                    device: offer.device.clone(),
                                                    maker: offer.maker_id.clone(),
                                                    role: offer.role.clone(),
                                                    version: crate::firmware::VersionPolicy::Latest,
                                                },
                                            );
                                        }
                                        flasher_manage_draft.set(draft);
                                    }
                                    // Turning off leaves individual checks as-is (plan).
                                }
                            }
                            "Download all (Latest)"
                        }
                        if flasher_download_all() {
                            p { class: "mb-3 text-xs text-amber-200",
                                "Full set is {full_set_label}. Disk can grow further if Previous versions are kept too."
                            }
                        }
                        p { class: "mb-2 text-xs text-muted-foreground",
                            "Click a board to expand its roles. Cards stay collapsed so the list stays responsive."
                        }
                        div { class: "fw-manage-list mb-3",
                            if manage_groups.is_empty() {
                                p { class: "p-3 text-sm text-muted-foreground",
                                    if catalog_missing {
                                        "Sync once to fetch the flasher catalog, then choose boards here."
                                    } else {
                                        "No boards match that search."
                                    }
                                }
                            } else {
                                for (_score, device_name, maker_label, roles) in manage_groups.iter() {
                                    {
                                        let card_key = device_name.clone();
                                        let card_toggle = card_key.clone();
                                        let expanded = flasher_manage_expanded().contains(&card_key);
                                        let draft = flasher_manage_draft();
                                        let selected_n = roles
                                            .iter()
                                            .filter(|o| draft.contains_key(&o.key))
                                            .count();
                                        let role_n = roles.len();
                                        let chevron = if expanded { "▾" } else { "▸" };
                                        rsx! {
                                            div {
                                                key: "{card_key}",
                                                class: "fw-manage-card",
                                                button {
                                                    class: "fw-manage-card-head fw-manage-card-head-btn",
                                                    r#type: "button",
                                                    onclick: move |_| {
                                                        let mut open = flasher_manage_expanded();
                                                        if open.contains(&card_toggle) {
                                                            open.remove(&card_toggle);
                                                        } else {
                                                            open.insert(card_toggle.clone());
                                                        }
                                                        flasher_manage_expanded.set(open);
                                                    },
                                                    span { class: "fw-manage-card-chevron", "{chevron}" }
                                                    div { class: "min-w-0 flex-1",
                                                        p { class: "fw-manage-card-title", "{device_name}" }
                                                        p { class: "fw-manage-card-maker", "{maker_label}" }
                                                    }
                                                    span { class: "fw-manage-card-count",
                                                        "{selected_n}/{role_n} roles"
                                                    }
                                                }
                                                if expanded {
                                                    for (role_idx, offer) in roles.iter().enumerate() {
                                                        {
                                                            let key = offer.key.clone();
                                                            let key_toggle = key.clone();
                                                            let key_ver = key.clone();
                                                            let device = offer.device.clone();
                                                            let maker_id = offer.maker_id.clone();
                                                            let role = offer.role.clone();
                                                            let pill = offer.role_pill.clone();
                                                            let family_tags = flasher_manage_family_tags()
                                                                .get(&offer.family)
                                                                .cloned()
                                                                .unwrap_or_default();
                                                            let latest_label = crate::firmware::version_policy_option_label(
                                                                crate::firmware::VersionPolicy::Latest,
                                                                &family_tags,
                                                                &offer.static_versions,
                                                            );
                                                            let previous_label = crate::firmware::version_policy_option_label(
                                                                crate::firmware::VersionPolicy::Previous,
                                                                &family_tags,
                                                                &offer.static_versions,
                                                            );
                                                            let checked = draft.contains_key(&key);
                                                            let version = draft
                                                                .get(&key)
                                                                .map(|e| e.version)
                                                                .unwrap_or(crate::firmware::VersionPolicy::Latest);
                                                            let on_disk = on_disk_keys.contains(&key);
                                                            let unpublished = unavailable_keys.contains(&key);
                                                            let row_class = if role_idx % 2 == 0 {
                                                                "fw-manage-role-row fw-manage-role-row-even"
                                                            } else {
                                                                "fw-manage-role-row fw-manage-role-row-odd"
                                                            };
                                                            let offers_len = flasher_manage_offers().len();
                                                            rsx! {
                                                                div {
                                                                    key: "{key}",
                                                                    class: "{row_class}",
                                                                    input {
                                                                        r#type: "checkbox",
                                                                        checked: checked,
                                                                        oninput: move |_| {
                                                                            let mut draft = flasher_manage_draft();
                                                                            if draft.contains_key(&key_toggle) {
                                                                                draft.remove(&key_toggle);
                                                                                flasher_download_all.set(false);
                                                                            } else {
                                                                                draft.insert(
                                                                                    key_toggle.clone(),
                                                                                    crate::firmware::SelectionEntry {
                                                                                        key: key_toggle.clone(),
                                                                                        device: device.clone(),
                                                                                        maker: maker_id.clone(),
                                                                                        role: role.clone(),
                                                                                        version: crate::firmware::VersionPolicy::Latest,
                                                                                    },
                                                                                );
                                                                                if offers_len > 0
                                                                                    && draft.len() == offers_len
                                                                                {
                                                                                    flasher_download_all.set(true);
                                                                                }
                                                                            }
                                                                            flasher_manage_draft.set(draft);
                                                                        }
                                                                    }
                                                                    span { class: "fw-manage-pill", "{pill}" }
                                                                    select {
                                                                        class: "fw-manage-version",
                                                                        value: "{version.as_str()}",
                                                                        disabled: !checked,
                                                                        onchange: move |event: FormEvent| {
                                                                            let policy = crate::firmware::VersionPolicy::parse(&event.value());
                                                                            let mut draft = flasher_manage_draft();
                                                                            if let Some(entry) = draft.get_mut(&key_ver) {
                                                                                entry.version = policy;
                                                                            }
                                                                            flasher_manage_draft.set(draft);
                                                                        },
                                                                        option {
                                                                            value: "latest",
                                                                            style: "background-color:#151a20;color:#f4f4f5;",
                                                                            "{latest_label}"
                                                                        }
                                                                        option {
                                                                            value: "previous",
                                                                            style: "background-color:#151a20;color:#f4f4f5;",
                                                                            "{previous_label}"
                                                                        }
                                                                    }
                                                                    span { class: "fw-manage-status",
                                                                        if on_disk {
                                                                            span { class: "fw-manage-status-on", "On disk" }
                                                                        } else if unpublished {
                                                                            span { class: "fw-manage-status-off", title: "MeshCore did not publish this role for the selected release",
                                                                                "Not published"
                                                                            }
                                                                        } else {
                                                                            span { class: "fw-manage-status-off", "Missing" }
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
                    }
                    Card { title: "USB flash".to_string(), icon: Some(IconName::Radio),
                        p { class: "mb-3 text-sm text-muted-foreground",
                            "Flash from the local cache only. ESP32 needs esptool on PATH; nRF52 needs adafruit-nrfutil or a manual UF2 copy. Disconnect the phone MeshCore app. Leave Erase off to keep the device identity."
                        }
                        div { class: "mb-3",
                            TextInput {
                                value: flasher_query(),
                                placeholder: "Filter devices…",
                                oninput: move |event: FormEvent| flasher_query.set(event.value()),
                            }
                        }
                        div { class: "mb-3 grid gap-3 sm:grid-cols-2",
                            div {
                                label { class: "mb-1 block text-xs text-muted-foreground", "Device" }
                                SelectInput {
                                    id: "flasher-device".to_string(),
                                    value: flasher_device(),
                                    onchange: move |event: FormEvent| {
                                        flasher_device.set(event.value());
                                        flasher_role.set(String::new());
                                        flasher_tag.set(String::new());
                                    },
                                    if filtered.is_empty() {
                                        option {
                                            value: "",
                                            style: "background-color:#151a20;color:#f4f4f5;",
                                            "Sync catalog first"
                                        }
                                    }
                                    for device in filtered.iter() {
                                        option {
                                            key: "{device.name}",
                                            value: "{device.name}",
                                            style: "background-color:#151a20;color:#f4f4f5;",
                                            "{device.name}"
                                        }
                                    }
                                }
                            }
                            div {
                                label { class: "mb-1 block text-xs text-muted-foreground", "Role" }
                                SelectInput {
                                    id: "flasher-role".to_string(),
                                    value: flasher_role(),
                                    onchange: move |event: FormEvent| {
                                        flasher_role.set(event.value());
                                        flasher_tag.set(String::new());
                                    },
                                    if let Some(dev) = selected_device.as_ref() {
                                        for role in dev.roles.iter() {
                                            option {
                                                key: "{role.role}",
                                                value: "{role.role}",
                                                style: "background-color:#151a20;color:#f4f4f5;",
                                                "{role.label}"
                                            }
                                        }
                                    } else {
                                        option {
                                            value: "",
                                            style: "background-color:#151a20;color:#f4f4f5;",
                                            "—"
                                        }
                                    }
                                }
                            }
                        }
                        if let Some(role) = selected_role.as_ref() {
                            if !role.tooltip.is_empty() {
                                p { class: "mb-3 text-xs text-muted-foreground", "{role.tooltip}" }
                            }
                        }
                        div { class: "mb-3 grid gap-3 sm:grid-cols-2",
                            div {
                                label { class: "mb-1 block text-xs text-muted-foreground", "Version (local)" }
                                SelectInput {
                                    id: "flasher-tag".to_string(),
                                    value: flasher_tag(),
                                    onchange: move |event: FormEvent| flasher_tag.set(event.value()),
                                    if version_options.is_empty() {
                                        option {
                                            value: "",
                                            style: "background-color:#151a20;color:#f4f4f5;",
                                            "No matching local file"
                                        }
                                    }
                                    for pair in version_options.iter() {
                                        option {
                                            key: "{pair.0}-{pair.1}",
                                            value: "{pair.0}",
                                            style: "background-color:#151a20;color:#f4f4f5;",
                                            "{pair.0}"
                                        }
                                    }
                                }
                            }
                            div {
                                label { class: "mb-1 block text-xs text-muted-foreground", "USB port" }
                                div { class: "flex gap-2",
                                    SelectInput {
                                        id: "flasher-port".to_string(),
                                        value: flasher_port(),
                                        onchange: move |event: FormEvent| flasher_port.set(event.value()),
                                        if ports.is_empty() {
                                            option {
                                                value: "",
                                                style: "background-color:#151a20;color:#f4f4f5;",
                                                "No USB serial ports"
                                            }
                                        }
                                        for port in ports.iter() {
                                            option {
                                                key: "{port.path}",
                                                value: "{port.path}",
                                                style: "background-color:#151a20;color:#f4f4f5;",
                                                "{port.label}"
                                            }
                                        }
                                    }
                                    Button {
                                        kind: ButtonKind::Ghost,
                                        disabled: radio.state == "scanning_usb",
                                        onclick: move |_| {
                                            state().radio.scan_usb();
                                            flasher_tick.with_mut(|n| *n = n.wrapping_add(1));
                                        },
                                        if radio.state == "scanning_usb" {
                                            Throbber { size: 14 }
                                        } else {
                                            Icon { name: IconName::Scan, size: 14 }
                                        }
                                        if radio.state == "scanning_usb" { "Refreshing…" } else { "Refresh" }
                                    }
                                }
                            }
                        }
                        label { class: "mb-3 flex items-center gap-2 text-sm",
                            input {
                                r#type: "checkbox",
                                checked: flasher_erase(),
                                oninput: move |_| {
                                    let next = !flasher_erase();
                                    flasher_erase.set(next);
                                    if !next {
                                        flasher_erase_armed.set(false);
                                    }
                                }
                            }
                            "Erase device (full wipe — new MeshCore identity)"
                        }
                        if flasher_erase() && !flasher_erase_armed() {
                            p { class: "mb-3 text-xs text-amber-200",
                                "Erase replaces the public key. Contacts must re-add this node. Tick Confirm erase below before flashing."
                            }
                            label { class: "mb-3 flex items-center gap-2 text-sm",
                                input {
                                    r#type: "checkbox",
                                    checked: flasher_erase_armed(),
                                    oninput: move |_| flasher_erase_armed.set(!flasher_erase_armed())
                                }
                                "Confirm erase"
                            }
                        }
                        div { class: "flex flex-wrap gap-2",
                            Button {
                                disabled: flash_busy
                                    || sync_busy
                                    || asset_ready.is_none()
                                    || flasher_port().trim().is_empty()
                                    || (flasher_erase() && !flasher_erase_armed()),
                                onclick: move |_| {
                                    let Some(role) = selected_role.clone() else { return; };
                                    let Ok((path, uf2)) = crate::firmware::resolve_flash_asset(
                                        &crate::firmware::load_index(),
                                        &role,
                                        &flasher_tag(),
                                        flasher_erase(),
                                    ) else {
                                        return;
                                    };
                                    let platform = selected_device
                                        .as_ref()
                                        .map(|d| d.platform.clone())
                                        .unwrap_or_else(|| "esp32".into());
                                    crate::firmware::start_flash(
                                        state().radio.clone(),
                                        crate::firmware::FlashRequest {
                                            platform,
                                            port: flasher_port(),
                                            asset_path: path,
                                            erase: flasher_erase(),
                                            uf2,
                                        },
                                    );
                                    flasher_tick.with_mut(|n| *n = n.wrapping_add(1));
                                },
                                Icon { name: IconName::Radio, size: 14 }
                                if flash_busy { "Flashing…" } else { "Flash" }
                            }
                        }
                        if !flash.detail.is_empty() {
                            p { class: "mt-3 text-sm text-muted-foreground", "{flash.detail}" }
                        }
                        if !flash.log.is_empty() {
                            pre { class: "mt-2 max-h-48 overflow-auto rounded-lg border border-border bg-background p-3 font-mono text-xs whitespace-pre-wrap",
                                "{flash.log}"
                            }
                        }
                    }
                }
            }
            }

            if settings_tab() == SettingsTab::Diagnostics {
                Card { title: "App log".to_string(), icon: Some(IconName::Messages),
                    p { class: "mb-3 text-sm text-muted-foreground",
                        "Live diagnostic lines from radio connect, sync, send, and CLI. Separate from the security audit trail. Secrets and PINs are redacted. Keeps the last 1000 lines."
                    }
                    div { class: "mb-3 flex flex-wrap items-end gap-2",
                        div { class: "min-w-[10rem]",
                            label { class: "mb-1 block text-xs text-muted-foreground", "Level" }
                            SelectInput {
                                id: "log-filter".to_string(),
                                value: log_filter(),
                                onchange: move |event: FormEvent| log_filter.set(event.value()),
                                for filter in [
                                    crate::app_log::LogFilter::All,
                                    crate::app_log::LogFilter::Error,
                                    crate::app_log::LogFilter::Warn,
                                    crate::app_log::LogFilter::Info,
                                    crate::app_log::LogFilter::Debug,
                                    crate::app_log::LogFilter::Verbose,
                                ] {
                                    option {
                                        key: "{filter.value()}",
                                        value: "{filter.value()}",
                                        style: "background-color:#101418;color:#f4f4f5;",
                                        "{filter.label()}"
                                    }
                                }
                            }
                        }
                        div { class: "min-w-[14rem] flex-1",
                            label { class: "mb-1 block text-xs text-muted-foreground", "Fuzzy search" }
                            TextInput {
                                value: log_query(),
                                placeholder: "send, sync, BLE…",
                                oninput: move |event: FormEvent| log_query.set(event.value()),
                            }
                        }
                        label { class: "mb-0.5 flex cursor-pointer items-center gap-2 text-xs text-muted-foreground",
                            input {
                                r#type: "checkbox",
                                checked: log_autoscroll(),
                                oninput: move |_| log_autoscroll.set(!log_autoscroll()),
                            }
                            "Auto-scroll"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| {
                                crate::app_log::clear();
                                log_tick.with_mut(|n| *n = n.wrapping_add(1));
                            },
                            "Clear"
                        }
                    }
                    pre {
                        id: "hq-log-transcript",
                        class: "log-transcript",
                        if log_rows.is_empty() {
                            span { class: "text-muted-foreground", "No log lines yet for this filter.\n" }
                        }
                        for entry in log_rows.iter() {
                            {
                                let tone = match entry.level {
                                    crate::app_log::LogLevel::Error => "log-line-error",
                                    crate::app_log::LogLevel::Warn => "log-line-warn",
                                    crate::app_log::LogLevel::Info => "log-line-info",
                                    crate::app_log::LogLevel::Debug => "log-line-debug",
                                    crate::app_log::LogLevel::Verbose => "log-line-verbose",
                                };
                                let line = format!(
                                    "{}  {:<7}  {}\n",
                                    crate::app_log::format_time(entry.at),
                                    entry.level.as_str(),
                                    entry.message
                                );
                                rsx! {
                                    span {
                                        key: "{entry.id}",
                                        class: "{tone}",
                                        "{line}"
                                    }
                                }
                            }
                        }
                    }
                    p { class: "mt-2 text-xs text-muted-foreground",
                        "{log_rows.len()} shown · filter {crate::app_log::LogFilter::from_value(&log_filter()).label()}"
                        if log_autoscroll() { " · auto-scroll on" } else { " · auto-scroll off" }
                    }
                }
            }

            if settings_tab() == SettingsTab::About {
                Card { title: "About".to_string(), icon: Some(IconName::Settings),
                    p { class: "mb-2 text-sm text-muted-foreground",
                        "Local offline emergency headquarters for MeshCore communities. No cloud account — this computer owns the radio link, roster, and bot."
                    }
                    div { class: "mb-3",
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| on_open_help.call(()),
                            Icon { name: IconName::Help, size: 14 }
                            "Open Help"
                        }
                    }
                    {
                        let version = env!("CARGO_PKG_VERSION");
                        let data_path = crate::data_dir().display().to_string();
                        let data_path_copy = data_path.clone();
                        let repo = "https://github.com/digitalexpl0it/MetisMesh-HQ";
                        rsx! {
                            div { class: "about-meta",
                                div { class: "about-row",
                                    span { class: "about-key", "App" }
                                    span { class: "about-val", "Metis Mesh HQ" }
                                    span {}
                                }
                                div { class: "about-row",
                                    span { class: "about-key", "Version" }
                                    span { class: "about-val", "{version}" }
                                    span {}
                                }
                                div { class: "about-row",
                                    span { class: "about-key", "Author" }
                                    span { class: "about-val", "DigitalExpl0it" }
                                    span {}
                                }
                                div { class: "about-row",
                                    span { class: "about-key", "License" }
                                    span { class: "about-val", "MIT" }
                                    span {}
                                }
                                div { class: "about-row",
                                    span { class: "about-key", "Source" }
                                    span { class: "about-val", "{repo}" }
                                    Button {
                                        kind: ButtonKind::Ghost,
                                        onclick: move |_| copy_text_to_clipboard(repo),
                                        Icon { name: IconName::Copy, size: 12 }
                                        "Copy"
                                    }
                                }
                                div { class: "about-row",
                                    span { class: "about-key", "Data folder" }
                                    span { class: "about-val", "{data_path}" }
                                    Button {
                                        kind: ButtonKind::Ghost,
                                        onclick: move |_| copy_text_to_clipboard(&data_path_copy),
                                        Icon { name: IconName::Copy, size: 12 }
                                        "Copy"
                                    }
                                }
                            }
                            p { class: "mb-3 text-xs text-muted-foreground",
                                "Credits: Dioxus, MapLibre, MeshCore / meshcore-rs."
                            }
                            div { class: "flex flex-wrap gap-2",
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| {
                                        reopen_welcome();
                                        status_note("Setup wizard will open again.");
                                    },
                                    "Run setup again"
                                }
                            }
                        }
                    }
                }
                Card { title: "Offline runbook (first hour)".to_string(), icon: Some(IconName::Help),
                    p { class: "mb-2 text-sm text-muted-foreground",
                        "For volunteers when the Internet is down. The full runbook (before the outage, first hour, UPS tips, do/don’t) lives in the app under Help → Offline runbook — no separate markdown viewer needed."
                    }
                    ol { class: "mb-3 list-decimal space-y-1 pl-5 text-sm text-foreground",
                        li { "Power the laptop (AC or UPS) and the companion radio. Disable sleep." }
                        li { "Open Metis Mesh HQ — no cloud login." }
                        li { "Settings → Connect: confirm the companion is connected (Refresh USB if the port moved)." }
                        li { "Open Map, then Messages — post a short channel check-in so the room knows HQ is live." }
                        li { "Roster: approve only people you trust. Secrets never appear in the message list." }
                        li { "Flash only from Settings → Flasher after Sync shows Ready for outage (or Import pack from a USB .zip)." }
                    }
                    p { class: "mb-3 text-sm text-muted-foreground",
                        "Battery / UPS: size for laptop + USB radio (often 2–4 hours). Keep vents clear. Mesh traffic does not need Internet once firmware is already synced. Do not present HQ traffic as FEMA, NWS, or law enforcement."
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| on_open_help.call(()),
                        Icon { name: IconName::Help, size: 14 }
                        "Open offline runbook in Help"
                    }
                }
            }

            Dialog {
                open: confirm_purge(),
                title: "Purge history".to_string(),
                on_close: move |_| confirm_purge.set(false),
                if let Some((cutoff, preview)) = preview {
                    p { class: "mb-3 text-sm text-muted-foreground",
                        "This deletes {preview.messages} messages, {preview.paths} routes, {preview.positions} location trails, {preview.telemetry} telemetry samples, and {preview.infra_status} repeater status samples older than the retention window. Roster, secrets, and the audit log stay."
                    }
                    div { class: "flex gap-2",
                        Button {
                            kind: ButtonKind::Danger,
                            onclick: move |_| {
                                match state().db.purge(cutoff) {
                                    Ok(done) => purge_note.set(format!(
                                        "Purged {} messages, {} routes, {} positions.",
                                        done.messages, done.paths, done.positions
                                    )),
                                    Err(err) => purge_note.set(err.to_string()),
                                }
                                confirm_purge.set(false);
                            },
                            Icon { name: IconName::Trash, size: 14 }
                            "Delete"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| confirm_purge.set(false),
                            Icon { name: IconName::Close, size: 14 }
                            "Cancel"
                        }
                    }
                } else {
                    p { class: "text-sm text-muted-foreground", "Set a retention window before purging." }
                }
            }

            Dialog {
                open: confirm_infra_purge(),
                title: "Purge repeater status".to_string(),
                on_close: move |_| confirm_infra_purge.set(false),
                if let Some((cutoff, count)) = infra_preview {
                    p { class: "mb-3 text-sm text-muted-foreground",
                        "This deletes {count} repeater status sample(s) older than the infrastructure retention window. Latest snapshot cards, roster, and claims stay."
                    }
                    div { class: "flex gap-2",
                        Button {
                            kind: ButtonKind::Danger,
                            onclick: move |_| {
                                match state().db.purge_infra_status(cutoff) {
                                    Ok(n) => infra_purge_note.set(format!(
                                        "Deleted {n} repeater status sample(s)."
                                    )),
                                    Err(err) => infra_purge_note.set(err.to_string()),
                                }
                                confirm_infra_purge.set(false);
                            },
                            Icon { name: IconName::Trash, size: 14 }
                            "Delete"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| confirm_infra_purge.set(false),
                            Icon { name: IconName::Close, size: 14 }
                            "Cancel"
                        }
                    }
                } else {
                    p { class: "text-sm text-muted-foreground",
                        "Set infrastructure retention to 7 / 30 / 90 days before purging."
                    }
                }
            }

            Dialog {
                open: restore_preview().is_some(),
                title: "Restore backup".to_string(),
                on_close: move |_| restore_preview.set(None),
                if let Some((path, pass, preview)) = restore_preview() {
                    p { class: "mb-3 text-sm text-muted-foreground",
                        "Replace this HQ with backup from {preview.hq_name} (app {preview.app_version}, created {preview.created_at}). "
                        "{preview.node_count} nodes, {preview.message_count} messages, {preview.channel_count} channels, {preview.secret_count} secrets. "
                        "File: {path}. Restart recommended if a radio is connected."
                    }
                    div { class: "flex gap-2",
                        Button {
                            kind: ButtonKind::Danger,
                            onclick: move |_| {
                                match crate::backup::decrypt_backup_file(
                                    std::path::Path::new(&path),
                                    &pass,
                                ) {
                                    Ok(payload) => {
                                        match crate::backup::apply_restore(
                                            &state().db,
                                            &state().secrets,
                                            &payload,
                                        ) {
                                            Ok(()) => {
                                                let _ = state().db.audit(
                                                    "restore",
                                                    "",
                                                    &path,
                                                );
                                                backup_note.set(format!(
                                                    "Restored from {path}. Restart recommended if radio was connected."
                                                ));
                                                status_note("HQ restored from backup");
                                                restore_pass.set(String::new());
                                            }
                                            Err(err) => backup_note.set(err.to_string()),
                                        }
                                    }
                                    Err(err) => backup_note.set(err.to_string()),
                                }
                                restore_preview.set(None);
                            },
                            "Replace HQ state"
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| restore_preview.set(None),
                            Icon { name: IconName::Close, size: 14 }
                            "Cancel"
                        }
                    }
                }
            }

            Dialog {
                open: confirm_contacts_import(),
                title: "Import contacts".to_string(),
                on_close: move |_| confirm_contacts_import.set(false),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "{contacts_import_preview()} Merge by public key into the HQ roster. Radio contact table is not changed."
                }
                div { class: "flex gap-2",
                    Button {
                        kind: ButtonKind::Primary,
                        onclick: move |_| {
                            let path = contacts_import_path();
                            match crate::contact_export::import_contacts_path(
                                &state().db,
                                std::path::Path::new(path.trim()),
                            ) {
                                Ok(summary) => {
                                    contacts_note.set(format!(
                                        "Import done: {} new, {} updated, {} skipped.",
                                        summary.new, summary.updated, summary.skipped
                                    ));
                                    status_note(format!(
                                        "Imported {} contacts",
                                        summary.new + summary.updated
                                    ));
                                }
                                Err(err) => contacts_note.set(err),
                            }
                            confirm_contacts_import.set(false);
                        },
                        "Import"
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| confirm_contacts_import.set(false),
                        Icon { name: IconName::Close, size: 14 }
                        "Cancel"
                    }
                }
            }

            {
                let _ = backups_tick();
                let download_list = crate::backup::list_backup_entries(&crate::data_dir());
                rsx! {
                    SlideDownPanel {
                        open: download_backups_open(),
                        title: "Download backups".to_string(),
                        subtitle: "Save a copy anywhere on this computer, or delete archives you no longer need.".to_string(),
                        icon: Some(IconName::Key),
                        on_close: move |_| download_backups_open.set(false),
                        if download_list.is_empty() {
                            p { class: "text-sm text-muted-foreground",
                                "No backups yet. Create one from Settings → Data first."
                            }
                        } else {
                            div { class: "flex flex-col gap-2",
                                for entry in download_list {
                                    {
                                        let path = entry.path.clone();
                                        let path_del = entry.path.clone();
                                        let path_s = entry.path_string();
                                        let path_clear = path_s.clone();
                                        let when = if entry.created_at > 0 {
                                            timefmt::format_stamp(&state().db, entry.created_at)
                                        } else {
                                            "Unknown date".into()
                                        };
                                        rsx! {
                                            div {
                                                key: "{path_s}",
                                                class: "flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border bg-background px-3 py-3",
                                                div { class: "min-w-0",
                                                    p { class: "font-medium", "{when}" }
                                                    p { class: "truncate font-mono text-xs text-muted-foreground", "{entry.file_name}" }
                                                }
                                                div { class: "flex flex-wrap gap-2",
                                                    Button {
                                                        kind: ButtonKind::Primary,
                                                        onclick: move |_| {
                                                            match crate::backup::export_backup_with_dialog(&path) {
                                                                Ok(dest) => {
                                                                    status_note(format!(
                                                                        "Backup saved to {}",
                                                                        dest.display()
                                                                    ));
                                                                    backup_note.set(format!(
                                                                        "Downloaded to {}",
                                                                        dest.display()
                                                                    ));
                                                                }
                                                                Err(err) => {
                                                                    let msg = err.to_string();
                                                                    if !msg.contains("cancelled") {
                                                                        status_note(msg.clone());
                                                                        backup_note.set(msg);
                                                                    }
                                                                }
                                                            }
                                                        },
                                                        Icon { name: IconName::Copy, size: 14 }
                                                        "Save as…"
                                                    }
                                                    Button {
                                                        kind: ButtonKind::Ghost,
                                                        onclick: move |_| {
                                                            match crate::backup::delete_backup(&path_del) {
                                                                Ok(()) => {
                                                                    let _ = state().db.audit(
                                                                        "backup_delete",
                                                                        "",
                                                                        &path_clear,
                                                                    );
                                                                    if restore_path() == path_clear {
                                                                        restore_path.set(String::new());
                                                                    }
                                                                    backup_note.set(format!("Deleted {path_clear}"));
                                                                    status_note("Backup deleted");
                                                                    backups_tick.with_mut(|n| *n = n.wrapping_add(1));
                                                                }
                                                                Err(err) => {
                                                                    status_note(err.to_string());
                                                                    backup_note.set(err.to_string());
                                                                }
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
                }
            }

            {
                let _ = exports_tick();
                let export_list = crate::contact_export::list_contact_exports(&crate::data_dir());
                rsx! {
                    SlideDownPanel {
                        open: download_exports_open(),
                        title: "Download contact exports".to_string(),
                        subtitle: "Save JSON exports anywhere on this computer, or delete ones you no longer need.".to_string(),
                        icon: Some(IconName::Users),
                        on_close: move |_| download_exports_open.set(false),
                        if export_list.is_empty() {
                            p { class: "text-sm text-muted-foreground",
                                "No contact exports yet. Export from Settings → Data first."
                            }
                        } else {
                            div { class: "flex flex-col gap-2",
                                for entry in export_list {
                                    {
                                        let path = entry.path.clone();
                                        let path_del = entry.path.clone();
                                        let path_s = entry.path_string();
                                        let path_clear = path_s.clone();
                                        let when = if entry.created_at > 0 {
                                            timefmt::format_stamp(&state().db, entry.created_at)
                                        } else {
                                            "Unknown date".into()
                                        };
                                        rsx! {
                                            div {
                                                key: "{path_s}",
                                                class: "flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border bg-background px-3 py-3",
                                                div { class: "min-w-0",
                                                    p { class: "font-medium", "{when}" }
                                                    p { class: "truncate font-mono text-xs text-muted-foreground", "{entry.file_name}" }
                                                }
                                                div { class: "flex flex-wrap gap-2",
                                                    Button {
                                                        kind: ButtonKind::Primary,
                                                        onclick: move |_| {
                                                            match crate::contact_export::export_contacts_with_dialog(&path)
                                                            {
                                                                Ok(dest) => {
                                                                    status_note(format!(
                                                                        "Export saved to {}",
                                                                        dest.display()
                                                                    ));
                                                                    contacts_note.set(format!(
                                                                        "Downloaded to {}",
                                                                        dest.display()
                                                                    ));
                                                                }
                                                                Err(err) => {
                                                                    if !err.contains("cancelled") {
                                                                        status_note(err.clone());
                                                                        contacts_note.set(err);
                                                                    }
                                                                }
                                                            }
                                                        },
                                                        Icon { name: IconName::Copy, size: 14 }
                                                        "Save as…"
                                                    }
                                                    Button {
                                                        kind: ButtonKind::Ghost,
                                                        onclick: move |_| {
                                                            match crate::contact_export::delete_contact_export(&path_del)
                                                            {
                                                                Ok(()) => {
                                                                    let _ = state().db.audit(
                                                                        "contacts_export_delete",
                                                                        "",
                                                                        &path_clear,
                                                                    );
                                                                    if contacts_import_path() == path_clear {
                                                                        contacts_import_path.set(String::new());
                                                                    }
                                                                    contacts_note.set(format!("Deleted {path_clear}"));
                                                                    status_note("Contact export deleted");
                                                                    exports_tick.with_mut(|n| *n = n.wrapping_add(1));
                                                                }
                                                                Err(err) => {
                                                                    status_note(err.clone());
                                                                    contacts_note.set(err);
                                                                }
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
                }
            }
        }
    }
}

#[component]
fn DeviceCard(
    device: BleRadio,
    selected: bool,
    connected: bool,
    paired: bool,
    on_use: EventHandler<MouseEvent>,
) -> Element {
    let border = if connected || device.companion {
        "border-emerald-500/40"
    } else if selected {
        "border-primary"
    } else {
        "border-border"
    };
    let icon_class = if device.companion {
        "mt-0.5 text-emerald-200"
    } else {
        "mt-0.5 text-muted-foreground"
    };
    rsx! {
        article { class: "flex items-start gap-3 rounded-lg border bg-background p-3 {border}",
            div { class: "{icon_class}",
                Icon {
                    name: if device.companion { IconName::Radio } else { IconName::Bluetooth },
                    size: 18,
                }
            }
            div { class: "min-w-0 flex-1",
                div { class: "flex flex-wrap items-center gap-2",
                    p { class: "truncate font-medium", "{device.name}" }
                    if device.companion {
                        Badge { tone: "enrolled".to_string(), "Companion" }
                    }
                    if paired {
                        Badge { tone: "ok".to_string(), "Saved" }
                    }
                    if connected {
                        Badge { tone: "ok".to_string(), "Connected" }
                    }
                }
                p { class: "mt-0.5 font-mono text-xs tracking-wide text-muted-foreground", "{device.address}" }
                p { class: "mt-1 flex items-center gap-1.5 text-xs text-muted-foreground",
                    Icon { name: IconName::Signal, size: 14 }
                    "{device.detail_line()}"
                }
            }
            if connected {
                span { class: "inline-flex h-9 items-center gap-1.5 text-xs text-emerald-200",
                    Icon { name: IconName::Check, size: 14 }
                    "In use"
                }
            } else if paired {
                Button {
                    kind: if selected { ButtonKind::Primary } else { ButtonKind::Ghost },
                    onclick: move |event| on_use.call(event),
                    Icon { name: IconName::Bluetooth, size: 14 }
                    "Connect"
                }
            } else {
                Button {
                    kind: if selected { ButtonKind::Primary } else { ButtonKind::Ghost },
                    onclick: move |event| on_use.call(event),
                    Icon { name: IconName::Key, size: 14 }
                    "Pair"
                }
            }
        }
    }
}

#[component]
fn RetentionChoice(
    label: &'static str,
    value: &'static str,
    current: String,
    onpick: EventHandler<String>,
) -> Element {
    let active = current == value;
    rsx! {
        Button {
            kind: if active { ButtonKind::Primary } else { ButtonKind::Ghost },
            onclick: move |_| onpick.call(value.to_string()),
            "{label}"
        }
    }
}

const MESSAGE_KEEP_OPTIONS: &[(&str, &str)] = &[
    ("1h", "1 hour"),
    ("6h", "6 hours"),
    ("24h", "24 hours"),
    ("7d", "1 week"),
    ("30d", "30 days"),
    ("forever", "Forever"),
];

fn message_keep_label(value: &str) -> &'static str {
    MESSAGE_KEEP_OPTIONS
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, label)| *label)
        .unwrap_or("Forever")
}

/// Keep-duration picker. Default is a compact dropdown; `inline` lists choices
/// in-flow (needed inside slide-downs that clip absolute menus).
#[component]
fn MessageKeepButton(
    current: String,
    #[props(default = false)] inline: bool,
    onpick: EventHandler<String>,
) -> Element {
    let mut open = use_signal(|| false);
    let label = message_keep_label(&current);
    let choices = rsx! {
        for (value, option_label) in MESSAGE_KEEP_OPTIONS {
            {
                let value = (*value).to_string();
                let active = current == value;
                rsx! {
                    button {
                        r#type: "button",
                        class: if active {
                            "keep-menu-item keep-menu-item-active"
                        } else {
                            "keep-menu-item"
                        },
                        role: "menuitemradio",
                        "aria-checked": if active { "true" } else { "false" },
                        onclick: move |_| {
                            onpick.call(value.clone());
                            open.set(false);
                        },
                        "{*option_label}"
                    }
                }
            }
        }
    };
    if inline {
        return rsx! {
            div {
                class: "keep-menu-panel keep-menu-panel-inline",
                role: "menu",
                "aria-label": "Default keep",
                {choices}
            }
        };
    }
    rsx! {
        div { class: "keep-menu-wrap",
            button {
                r#type: "button",
                class: "keep-menu-trigger",
                title: "Default message keep",
                onclick: move |event: MouseEvent| {
                    event.prevent_default();
                    event.stop_propagation();
                    open.set(!open());
                },
                Icon { name: IconName::Clock, size: 14 }
                span { "{label}" }
                Icon { name: IconName::ChevronDown, size: 12 }
            }
            if open() {
                PopoverDismissLayer {
                    on_dismiss: move |_| open.set(false),
                }
                div {
                    class: "keep-menu-panel",
                    role: "menu",
                    "aria-label": "Default keep",
                    onclick: move |event: MouseEvent| event.stop_propagation(),
                    {choices}
                }
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
enum AppearanceEditTarget {
    Global,
    Channel(String),
}

/// Edit global or per-channel Messages pane appearance.
#[component]
fn AppearanceEditor(
    target: Signal<Option<AppearanceEditTarget>>,
    on_saved: EventHandler<()>,
) -> Element {
    use crate::messages_appearance::{
        BgAngle, BgMode, MessagesAppearance, TextSize, BG_OPACITY_MAX,
    };
    let mut target = target;
    let open = target().is_some();
    let mut draft = use_signal(MessagesAppearance::default);
    let mut loaded_for = use_signal(|| None::<AppearanceEditTarget>);
    if open {
        let current = target();
        if current != loaded_for() {
            let next = match current.clone() {
                Some(AppearanceEditTarget::Global) => {
                    state().db.messages_appearance().unwrap_or_default()
                }
                Some(AppearanceEditTarget::Channel(name)) => state()
                    .db
                    .channel_appearance(&name)
                    .ok()
                    .flatten()
                    .or_else(|| state().db.messages_appearance().ok())
                    .unwrap_or_default(),
                None => MessagesAppearance::default(),
            };
            draft.set(next);
            loaded_for.set(current);
        }
    } else if loaded_for().is_some() {
        loaded_for.set(None);
    }
    let title = match target() {
        Some(AppearanceEditTarget::Global) => "Messages appearance".to_string(),
        Some(AppearanceEditTarget::Channel(name)) => format!("Appearance · {name}"),
        None => "Appearance".to_string(),
    };
    let is_channel = matches!(target(), Some(AppearanceEditTarget::Channel(_)));
    let preview = draft();
    let preview_style = preview.pane_style();
    rsx! {
        Dialog {
            open: open,
            title: title,
            on_close: move |_| target.set(None),
            div { class: "flex flex-col gap-3",
                p { class: "text-sm text-muted-foreground",
                    if is_channel {
                        "Overrides the global Messages look for this channel only. HQ-local — not sent on the mesh."
                    } else {
                        "Default look for channel panes (and DMs). Each channel can override. HQ-local — not sent on the mesh."
                    }
                }
                div { class: "appearance-preview", style: "{preview_style}",
                    span { "Preview · The quick brown fox" }
                }
                div { class: "flex flex-wrap items-center justify-between gap-2",
                    span { class: "text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Background" }
                    div { class: "appearance-seg",
                        button {
                            class: if draft().bg_mode == BgMode::Solid { "appearance-seg-active" } else { "" },
                            onclick: move |_| {
                                let mut d = draft();
                                d.bg_mode = BgMode::Solid;
                                draft.set(d);
                            },
                            "Solid"
                        }
                        button {
                            class: if draft().bg_mode == BgMode::Gradient { "appearance-seg-active" } else { "" },
                            onclick: move |_| {
                                let mut d = draft();
                                d.bg_mode = BgMode::Gradient;
                                draft.set(d);
                            },
                            "Gradient"
                        }
                    }
                }
                div { class: "grid gap-2 sm:grid-cols-2",
                    label { class: "flex flex-col gap-1 text-xs text-muted-foreground",
                        "Color A"
                        div { class: "flex items-center gap-2",
                            input {
                                r#type: "color",
                                class: "h-9 w-12 cursor-pointer rounded border border-border bg-card",
                                value: "{draft().bg_color}",
                                oninput: move |event: FormEvent| {
                                    let mut d = draft();
                                    d.bg_color = event.value();
                                    draft.set(d);
                                },
                            }
                            input {
                                class: "h-9 min-w-0 flex-1 rounded-md border border-border bg-card px-2 font-mono text-xs text-foreground",
                                value: "{draft().bg_color}",
                                oninput: move |event: FormEvent| {
                                    let mut d = draft();
                                    d.bg_color = event.value();
                                    draft.set(d);
                                },
                            }
                        }
                    }
                    if draft().bg_mode == BgMode::Gradient {
                        label { class: "flex flex-col gap-1 text-xs text-muted-foreground",
                            "Color B"
                            div { class: "flex items-center gap-2",
                                input {
                                    r#type: "color",
                                    class: "h-9 w-12 cursor-pointer rounded border border-border bg-card",
                                    value: "{draft().bg_color_b.clone().unwrap_or_else(|| \"#16324f\".into())}",
                                    oninput: move |event: FormEvent| {
                                        let mut d = draft();
                                        d.bg_color_b = Some(event.value());
                                        draft.set(d);
                                    },
                                }
                                input {
                                    class: "h-9 min-w-0 flex-1 rounded-md border border-border bg-card px-2 font-mono text-xs text-foreground",
                                    value: "{draft().bg_color_b.clone().unwrap_or_else(|| \"#16324f\".into())}",
                                    oninput: move |event: FormEvent| {
                                        let mut d = draft();
                                        d.bg_color_b = Some(event.value());
                                        draft.set(d);
                                    },
                                }
                            }
                        }
                    }
                }
                if draft().bg_mode == BgMode::Gradient {
                    div { class: "flex flex-wrap items-center justify-between gap-2",
                        span { class: "text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Angle" }
                        div { class: "appearance-seg",
                            button {
                                class: if draft().bg_angle == BgAngle::Vertical { "appearance-seg-active" } else { "" },
                                onclick: move |_| {
                                    let mut d = draft();
                                    d.bg_angle = BgAngle::Vertical;
                                    draft.set(d);
                                },
                                "Vertical"
                            }
                            button {
                                class: if draft().bg_angle == BgAngle::Diagonal { "appearance-seg-active" } else { "" },
                                onclick: move |_| {
                                    let mut d = draft();
                                    d.bg_angle = BgAngle::Diagonal;
                                    draft.set(d);
                                },
                                "Diagonal"
                            }
                        }
                    }
                }
                label { class: "flex flex-col gap-1 text-xs text-muted-foreground",
                    "Opacity · {draft().bg_opacity}% (max {BG_OPACITY_MAX})"
                    input {
                        r#type: "range",
                        min: "0",
                        max: "{BG_OPACITY_MAX}",
                        value: "{draft().bg_opacity}",
                        class: "w-full",
                        oninput: move |event: FormEvent| {
                            let mut d = draft();
                            d.bg_opacity = event.value().parse().unwrap_or(0).min(BG_OPACITY_MAX);
                            draft.set(d);
                        },
                    }
                }
                div { class: "flex flex-wrap items-center justify-between gap-2",
                    span { class: "text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground", "Text size" }
                    div { class: "appearance-seg",
                        button {
                            class: if draft().text_size == TextSize::Sm { "appearance-seg-active" } else { "" },
                            onclick: move |_| {
                                let mut d = draft();
                                d.text_size = TextSize::Sm;
                                draft.set(d);
                            },
                            "Sm"
                        }
                        button {
                            class: if draft().text_size == TextSize::Md { "appearance-seg-active" } else { "" },
                            onclick: move |_| {
                                let mut d = draft();
                                d.text_size = TextSize::Md;
                                draft.set(d);
                            },
                            "Md"
                        }
                        button {
                            class: if draft().text_size == TextSize::Lg { "appearance-seg-active" } else { "" },
                            onclick: move |_| {
                                let mut d = draft();
                                d.text_size = TextSize::Lg;
                                draft.set(d);
                            },
                            "Lg"
                        }
                    }
                }
                label { class: "flex flex-col gap-1 text-xs text-muted-foreground",
                    "Text color"
                    div { class: "flex flex-wrap items-center gap-2",
                        input {
                            r#type: "color",
                            class: "h-9 w-12 cursor-pointer rounded border border-border bg-card",
                            value: "{draft().text_color.clone().unwrap_or_else(|| \"#f4f4f5\".into())}",
                            oninput: move |event: FormEvent| {
                                let mut d = draft();
                                d.text_color = Some(event.value());
                                draft.set(d);
                            },
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| {
                                let mut d = draft();
                                d.text_color = None;
                                draft.set(d);
                            },
                            "Theme default"
                        }
                    }
                }
                div { class: "flex flex-wrap justify-end gap-2",
                    if is_channel {
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| {
                                if let Some(AppearanceEditTarget::Channel(name)) = target() {
                                    match state().db.set_channel_appearance(&name, None) {
                                        Ok(()) => {
                                            target.set(None);
                                            status_note(format!("{name} appearance reset to global."));
                                            on_saved.call(());
                                        }
                                        Err(err) => status_note(err.to_string()),
                                    }
                                }
                            },
                            "Reset to global"
                        }
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| target.set(None),
                        "Cancel"
                    }
                    Button {
                        onclick: move |_| {
                            let appearance = draft().clamp();
                            match target() {
                                Some(AppearanceEditTarget::Global) => {
                                    match state().db.set_messages_appearance(&appearance) {
                                        Ok(()) => {
                                            target.set(None);
                                            status_note("Messages appearance saved.");
                                            on_saved.call(());
                                        }
                                        Err(err) => status_note(err.to_string()),
                                    }
                                }
                                Some(AppearanceEditTarget::Channel(name)) => {
                                    match state().db.set_channel_appearance(&name, Some(&appearance)) {
                                        Ok(()) => {
                                            target.set(None);
                                            status_note(format!("{name} appearance saved."));
                                            on_saved.call(());
                                        }
                                        Err(err) => status_note(err.to_string()),
                                    }
                                }
                                None => target.set(None),
                            }
                        },
                        "Save"
                    }
                }
            }
        }
    }
}

/// Per-channel keep override (`default` = inherit global `message_keep`).
#[component]
fn ChannelKeepSelect(
    channel_name: String,
    current: String,
    on_done: EventHandler<String>,
) -> Element {
    let select_id = format!("channel-keep-{}", channel_name.replace(' ', "-"));
    rsx! {
        div { class: "channel-keep-select",
            label {
                class: "channel-keep-select__label",
                r#for: "{select_id.clone()}",
                "Keep"
            }
            div { class: "channel-keep-select__control",
                SelectInput {
                    id: select_id.clone(),
                    value: current.clone(),
                    onchange: {
                        let channel_name = channel_name.clone();
                        move |event: FormEvent| {
                            let value = event.value();
                            let keep = if value == "default" {
                                None
                            } else {
                                Some(value.as_str())
                            };
                            match state().db.set_channel_message_keep(&channel_name, keep) {
                                Ok(()) => {
                                    let _ = state().db.apply_message_keep();
                                    let label = match value.as_str() {
                                        "default" => "default".into(),
                                        other => other.to_string(),
                                    };
                                    on_done.call(format!("{channel_name} keep → {label}."));
                                }
                                Err(err) => on_done.call(err.to_string()),
                            }
                        }
                    },
                    option {
                        value: "default",
                        style: "background-color: #151a20; color: #f4f4f5;",
                        "Use default"
                    }
                    option {
                        value: "1h",
                        style: "background-color: #151a20; color: #f4f4f5;",
                        "1 hour"
                    }
                    option {
                        value: "6h",
                        style: "background-color: #151a20; color: #f4f4f5;",
                        "6 hours"
                    }
                    option {
                        value: "24h",
                        style: "background-color: #151a20; color: #f4f4f5;",
                        "24 hours"
                    }
                    option {
                        value: "7d",
                        style: "background-color: #151a20; color: #f4f4f5;",
                        "1 week"
                    }
                    option {
                        value: "30d",
                        style: "background-color: #151a20; color: #f4f4f5;",
                        "30 days"
                    }
                    option {
                        value: "forever",
                        style: "background-color: #151a20; color: #f4f4f5;",
                        "Forever"
                    }
                }
            }
        }
    }
}

fn print_contact_sheet(name: &str, pubkey: &str, uri: &str, qr_data_url: &str) {
    // Desktop WebKit blocks window.open popups; print via a same-document iframe.
    // Only data: QR URLs print reliably (metis:// is for the live UI protocol handler).
    if qr_data_url.is_empty() || !qr_data_url.starts_with("data:") {
        status_note("QR is not ready to print yet. Wait for the code to generate, then try again.");
        return;
    }
    let payload = serde_json::json!({
        "name": name,
        "pubkey": pubkey,
        "uri": uri,
        "qr": qr_data_url,
    });
    let script = format!(
        r#"(function(){{
  var d = {payload};
  function esc(s) {{
    return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
  }}
  var old = document.getElementById('metis-print-frame');
  if (old && old.parentNode) old.parentNode.removeChild(old);
  var iframe = document.createElement('iframe');
  iframe.id = 'metis-print-frame';
  iframe.setAttribute('aria-hidden', 'true');
  iframe.style.cssText = 'position:fixed;right:0;bottom:0;width:0;height:0;border:0;opacity:0;pointer-events:none;';
  document.body.appendChild(iframe);
  var doc = iframe.contentDocument || (iframe.contentWindow && iframe.contentWindow.document);
  if (!doc) {{
    document.body.removeChild(iframe);
    return false;
  }}
  var html = '<!DOCTYPE html><html><head><meta charset="utf-8"><title>' +
    esc(d.name) + ' — MeshCore contact</title><style>' +
    'html,body{{margin:0;padding:0;background:#fff;color:#0f172a}}' +
    'body{{font-family:system-ui,sans-serif;padding:2rem;text-align:center}}' +
    'h1{{font-size:1.35rem;margin:0 0 .35rem}}' +
    'p{{margin:.35rem 0;color:#475569}}' +
    'img{{width:240px;height:240px;margin:1.25rem auto;display:block;border:1px solid #e2e8f0}}' +
    'code{{font-size:11px;word-break:break-all;display:block;margin-top:1rem}}' +
    '.uri{{margin-top:1rem;font-size:11px;color:#64748b;word-break:break-all}}' +
    '@media print{{body{{padding:0.75rem}}}}' +
    '</style></head><body>' +
    '<h1>' + esc(d.name) + '</h1>' +
    '<p>Scan with MeshCore to add this contact</p>' +
    '<img src="' + String(d.qr).replace(/"/g,'&quot;') + '" alt="Contact QR"/>' +
    '<code>' + esc(d.pubkey) + '</code>' +
    '<p class="uri">' + esc(d.uri) + '</p>' +
    '</body></html>';
  doc.open();
  doc.write(html);
  doc.close();
  setTimeout(function(){{
    try {{
      var win = iframe.contentWindow;
      if (win) {{ win.focus(); win.print(); }}
    }} catch (e) {{}}
    setTimeout(function(){{
      var frame = document.getElementById('metis-print-frame');
      if (frame && frame.parentNode) frame.parentNode.removeChild(frame);
    }}, 1500);
  }}, 300);
  return true;
}})()"#,
        payload = payload
    );
    spawn(async move {
        let ok = document::eval(&format!("return {script};"))
            .await
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !ok {
            status_note("Could not open the print dialog. Check the desktop print permissions.");
        }
    });
}
