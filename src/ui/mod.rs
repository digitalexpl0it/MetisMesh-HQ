mod cli_page;
mod icons;
mod map_host;
mod map_page;
mod tracker_page;
mod widgets;

use std::sync::{Arc, OnceLock};

use chrono::{Local, TimeZone, Timelike};
use dioxus::prelude::*;

use crate::contact_qr::{
    contact_add_uri, enqueue_contact_qr, qr_data_url_from_file, qr_public_url,
};
use crate::db::{unix_now, Db, TelemetryPoint};
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
use icons::{Icon, IconName};
use map_page::MapPage;
use tracker_page::TrackerPage;
use widgets::{
    append_emoji, Badge, BarMix, BarMixRow, BatteryChart, BatteryPill, BatterySample, Button,
    ButtonKind, Card, ChartSeries, Dialog, EmojiPickerButton, LineChart, PageHeading, PinSlideDown,
    SelectInput, SlideDownPanel, Sparkline, TextInput, ToastHost,
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
    Tracker,
    Roster,
    Inbox,
    Messages,
    Cli,
    Settings,
}

#[allow(non_snake_case)]
pub fn App() -> Element {
    let mut page = use_signal(|| Page::Dashboard);
    let mut nav_open = use_signal(|| false);
    let mut pin_open = use_signal(|| false);
    let mut pin_target = use_signal(String::new);
    let mut pin_error = use_signal(String::new);
    let mut pin_form_key = use_signal(|| 0u64);
    let mut tick = use_signal(|| 0u64);
    let mut toast_tick = use_signal(|| 0u32);
    let mut started = use_signal(|| false);
    if !started() {
        started.set(true);
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
    let pending = state().db.pending_nodes().unwrap_or_default().len();
    let expanded = nav_open();
    let nav_width = if expanded { "w-56" } else { "w-[4.5rem]" };
    let page_title = match page() {
        Page::Dashboard => "Overview",
        Page::Map => "Map",
        Page::Tracker => "Tracker",
        Page::Roster => "Roster",
        Page::Inbox => "Inbox",
        Page::Messages => "Messages",
        Page::Cli => "CLI",
        Page::Settings => "Settings",
    };

    rsx! {
        document::Title { "Metis Mesh HQ" }
        // Desktop inlines these in with_custom_head so the first paint is styled.
        // Web still needs <link> injection here.
        if !cfg!(feature = "desktop") {
            document::Stylesheet { href: asset!("/assets/tailwind.css") }
            document::Stylesheet { href: asset!("/assets/form.css") }
        }
        div { class: "flex h-screen flex-col bg-background text-foreground",
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
            div { class: "flex min-h-0 flex-1",
                nav { class: "flex {nav_width} shrink-0 flex-col border-r border-border bg-card transition-[width] duration-200",
                    div { class: "flex items-center gap-3 border-b border-border px-3 py-3",
                        button {
                            class: "inline-flex h-11 w-11 shrink-0 items-center justify-center rounded-md text-signal hover:bg-accent cursor-pointer",
                            title: if expanded { "Collapse navigation" } else { "Expand navigation" },
                            onclick: move |_| nav_open.set(!nav_open()),
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
                    div { class: "flex flex-1 flex-col gap-1 p-2",
                        NavButton {
                            icon: IconName::Dashboard,
                            label: "Overview".to_string(),
                            expanded,
                            active: page() == Page::Dashboard,
                            onclick: move |_| page.set(Page::Dashboard),
                        }
                        NavButton {
                            icon: IconName::Map,
                            label: "Map".to_string(),
                            expanded,
                            active: page() == Page::Map,
                            onclick: move |_| page.set(Page::Map),
                        }
                        NavButton {
                            icon: IconName::Track,
                            label: "Tracker".to_string(),
                            expanded,
                            active: page() == Page::Tracker,
                            onclick: move |_| page.set(Page::Tracker),
                        }
                        NavButton {
                            icon: IconName::Users,
                            label: "Roster".to_string(),
                            expanded,
                            active: page() == Page::Roster,
                            onclick: move |_| page.set(Page::Roster),
                        }
                        NavButton {
                            icon: IconName::Inbox,
                            label: if pending > 0 { format!("Inbox ({pending})") } else { "Inbox".into() },
                            expanded,
                            active: page() == Page::Inbox,
                            badge: pending as u32,
                            onclick: move |_| page.set(Page::Inbox),
                        }
                        NavButton {
                            icon: IconName::Messages,
                            label: "Messages".to_string(),
                            expanded,
                            active: page() == Page::Messages,
                            onclick: move |_| page.set(Page::Messages),
                        }
                        NavButton {
                            icon: IconName::Radio,
                            label: "CLI".to_string(),
                            expanded,
                            active: page() == Page::Cli,
                            onclick: move |_| page.set(Page::Cli),
                        }
                        div { class: "mt-auto" }
                        NavButton {
                            icon: IconName::Settings,
                            label: "Settings".to_string(),
                            expanded,
                            active: page() == Page::Settings,
                            onclick: move |_| page.set(Page::Settings),
                        }
                    }
                }
                div { class: "flex min-w-0 flex-1 flex-col",
                    header { class: "flex h-14 shrink-0 items-center justify-between border-b border-border px-5",
                        div {
                            p { class: "text-[0.65rem] uppercase tracking-[0.18em] text-muted-foreground", "Metis Mesh HQ" }
                            h2 { class: "text-lg font-semibold leading-tight", "{page_title}" }
                        }
                        HeaderRadioControls { radio: radio.clone() }
                    }
                    main {
                        class: if page() == Page::Map {
                            "min-h-0 flex-1 overflow-hidden p-5"
                        } else {
                            "min-h-0 flex-1 overflow-auto p-5"
                        },
                        match page() {
                            Page::Dashboard => rsx! { DashboardPage { radio: radio.clone(), on_navigate: move |next| page.set(next) } },
                            Page::Map => rsx! { MapPage {} },
                            Page::Tracker => rsx! { TrackerPage {} },
                            Page::Roster => rsx! { RosterPage {} },
                            Page::Inbox => rsx! { InboxPage {} },
                            Page::Messages => rsx! { MessagesPage {} },
                            Page::Cli => rsx! { CliPage { radio: radio.clone() } },
                            Page::Settings => rsx! {
                                SettingsPage {
                                    radio: radio.clone(),
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
        "connecting" | "reconnecting" | "scanning" | "pairing" | "disconnecting" => IconName::Scan,
        "error" => IconName::Close,
        _ => IconName::Bluetooth,
    }
}

fn status_label(state: &str) -> &'static str {
    match state {
        "connected" => "Connected",
        "connecting" => "Connecting",
        "reconnecting" => "Reconnecting",
        "scanning" => "Scanning",
        "pairing" => "Pairing",
        "disconnecting" => "Disconnecting",
        "error" => "Radio error",
        _ => "Disconnected",
    }
}

#[component]
fn NavButton(
    icon: IconName,
    label: String,
    expanded: bool,
    active: bool,
    #[props(default)] badge: u32,
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
    rsx! {
        button {
            class: "{class} {layout} cursor-pointer",
            title: "{label}",
            onclick: move |event| onclick.call(event),
            if active {
                span { class: "absolute left-0 top-2 bottom-2 w-0.5 rounded-full bg-signal" }
            }
            Icon { name: icon, size: 24 }
            if expanded {
                span { class: "truncate text-sm font-medium", "{label}" }
            } else if badge > 0 {
                span { class: "absolute right-1.5 top-1.5 h-2 w-2 rounded-full bg-signal" }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TimeFormatPref {
    TwelveHour,
    TwentyFourHour,
}

impl TimeFormatPref {
    fn parse(value: &str) -> Self {
        match value.trim() {
            "24h" | "24" => Self::TwentyFourHour,
            _ => Self::TwelveHour,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::TwelveHour => "12h",
            Self::TwentyFourHour => "24h",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DateFormatPref {
    /// Oct 31
    MonthDayShort,
    /// Oct 31, 2026
    MonthDayYear,
    /// 10/31/2026
    MdyNumeric,
    /// 31/10/2026
    DmyNumeric,
    /// 2026-10-31
    YmdIso,
}

impl DateFormatPref {
    fn parse(value: &str) -> Self {
        match value.trim() {
            "mdy_long" | "month_day_year" => Self::MonthDayYear,
            "mdy" | "mdy_numeric" => Self::MdyNumeric,
            "dmy" | "dmy_numeric" => Self::DmyNumeric,
            "ymd" | "iso" => Self::YmdIso,
            _ => Self::MonthDayShort,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::MonthDayShort => "md_short",
            Self::MonthDayYear => "mdy_long",
            Self::MdyNumeric => "mdy",
            Self::DmyNumeric => "dmy",
            Self::YmdIso => "ymd",
        }
    }

    fn chrono_date(self) -> &'static str {
        // %-d / %-m drop leading zeros on Unix; Local already applies the computer TZ.
        match self {
            Self::MonthDayShort => "%b %-d",
            Self::MonthDayYear => "%b %-d, %Y",
            Self::MdyNumeric => "%-m/%-d/%Y",
            Self::DmyNumeric => "%-d/%-m/%Y",
            Self::YmdIso => "%Y-%m-%d",
        }
    }
}

fn load_time_format() -> TimeFormatPref {
    state()
        .db
        .setting("time_format")
        .ok()
        .flatten()
        .map(|value| TimeFormatPref::parse(&value))
        .unwrap_or(TimeFormatPref::TwelveHour)
}

fn load_date_format() -> DateFormatPref {
    state()
        .db
        .setting("date_format")
        .ok()
        .flatten()
        .map(|value| DateFormatPref::parse(&value))
        .unwrap_or(DateFormatPref::MonthDayShort)
}

fn format_time(seconds: Option<i64>) -> String {
    let Some(seconds) = seconds else {
        return "—".into();
    };
    let Some(time) = Local.timestamp_opt(seconds, 0).single() else {
        return seconds.to_string();
    };
    let date = load_date_format().chrono_date();
    let clock = match load_time_format() {
        TimeFormatPref::TwelveHour => "%-I:%M %p",
        TimeFormatPref::TwentyFourHour => "%H:%M",
    };
    time.format(&format!("{date} {clock}")).to_string()
}

fn format_clock(seconds: i64) -> String {
    let Some(time) = Local.timestamp_opt(seconds, 0).single() else {
        return seconds.to_string();
    };
    let clock = match load_time_format() {
        TimeFormatPref::TwelveHour => "%-I:%M %p",
        TimeFormatPref::TwentyFourHour => "%H:%M",
    };
    time.format(clock).to_string()
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
    companion_display_name(row, devices)
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
        "connecting" | "reconnecting" | "pairing" | "scanning" | "disconnecting"
    );
    let show_live = linked || busy || !radio.connected_name.is_empty();

    if show_live {
        let label = if !radio.connected_name.is_empty() {
            radio.connected_name.clone()
        } else {
            selected()
        };
        return rsx! {
            div { class: "inline-flex items-center gap-2",
                div { class: "inline-flex items-center gap-3 rounded-md border border-border bg-card px-3 py-1.5 text-xs text-muted-foreground",
                    span { class: "inline-flex min-w-0 items-center gap-2",
                        Icon { name: status_icon(&radio.state), size: 14 }
                        span { class: "text-foreground", "{status_label(&radio.state)}" }
                        if !label.is_empty() {
                            span { class: "max-w-[9rem] truncate text-muted-foreground", "· {label}" }
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
                    Icon { name: IconName::Bluetooth, size: 14 }
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
                        Icon { name: IconName::Radio, size: 14 }
                        span { class: "truncate text-foreground",
                            "{companion_display_name(row, &radio.devices)}"
                        }
                    }
                }
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
                    Icon { name: IconName::Bluetooth, size: 14 }
                    "Connect"
                }
            }
        }
    }
}

#[component]
fn DashboardPage(radio: RadioStatus, on_navigate: EventHandler<Page>) -> Element {
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
        "connecting" | "reconnecting" | "pairing" | "scanning" | "disconnecting"
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

    rsx! {
        div {
            class: "mx-auto flex w-full max-w-7xl flex-col gap-4 rounded-xl p-1",
            style: "background: linear-gradient(180deg, #0c0f12 0%, #12171c 100%);",
            PageHeading { icon: IconName::Dashboard, title: "Overview".to_string(),
                "Live status for the headquarters radio, roster, and recent mesh traffic."
            }
            div { class: "grid grid-cols-2 gap-3 lg:grid-cols-4",
                OverviewStat {
                    icon: IconName::Users,
                    label: "Enrolled".to_string(),
                    value: enrolled.to_string(),
                    hint: format!("{heard} heard, {total} known"),
                    tone: "signal".to_string(),
                    series: senders_hourly.clone(),
                    delta: last_hour_delta(&senders_hourly),
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
                    delta: last_hour_delta(&direct_hourly),
                }
                OverviewStat {
                    icon: IconName::Locate,
                    label: "GPS coverage".to_string(),
                    value: format!("{coverage}%"),
                    hint: format!("{with_gps} of {total} nodes with a fix"),
                    tone: "ok".to_string(),
                    series: position_hourly.clone(),
                    delta: last_hour_delta(&position_hourly),
                }
                OverviewStat {
                    icon: IconName::Messages,
                    label: "Recent traffic".to_string(),
                    value: messages_24h.to_string(),
                    hint: "Messages in the last 24h".to_string(),
                    tone: "muted".to_string(),
                    series: messages_hourly.clone(),
                    delta: last_hour_delta(&messages_hourly),
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
                        span { class: "spec-chip",
                            Icon { name: IconName::Radio, size: 12 }
                            span { class: "ml-1", "HQ" }
                        }
                    }
                    div { class: "flex items-start gap-3",
                        div { class: "flex h-11 w-11 shrink-0 items-center justify-center rounded-md bg-accent text-signal",
                            Icon { name: IconName::Bluetooth, size: 22 }
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
                                    "Companion linked · synced"
                                } else {
                                    "Pick a saved radio and connect"
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
                                "No saved companions yet. Pair one from Settings."
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
                                        Icon { name: IconName::Bluetooth, size: 14 }
                                        if busy { "Working" } else { "Connect" }
                                    }
                                }
                            }
                        }
                        Button {
                            kind: ButtonKind::Ghost,
                            onclick: move |_| on_navigate.call(Page::Settings),
                            Icon { name: IconName::Settings, size: 14 }
                            "Pair new radio"
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
                                Sparkline { values: traffic_tx.clone(), tone: "signal".to_string() }
                            }
                            div { class: "metric-tile",
                                div { class: "mb-1 flex items-center gap-1.5 text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground",
                                    Icon { name: IconName::Inbox, size: 12 }
                                    "Received · RX"
                                }
                                p { class: "text-xl font-semibold tabular-nums text-sky-200", "{total_rx_24h}" }
                                p { class: "mt-0.5 text-[0.65rem] text-muted-foreground", "Inbound last 24h" }
                                Sparkline { values: traffic_rx.clone(), tone: "ok".to_string() }
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
    #[props(default)] delta: Option<i64>,
) -> Element {
    let value_class = match tone.as_str() {
        "signal" => "text-signal",
        "pending" => "text-amber-200",
        "ok" => "text-sky-200",
        _ => "text-foreground",
    };
    rsx! {
        section { class: "rounded-lg border border-border bg-card px-3 py-2.5",
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
            Sparkline { values: series, tone }
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
    let mut details_pubkey = use_signal(|| None::<String>);
    let mut invite_channel = use_signal(|| {
        private_channels
            .first()
            .map(|row| row.name.clone())
            .unwrap_or_else(|| crate::secrets::CHANNEL_NAME.to_string())
    });
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
                    contact_type_label(node.contact_type),
                ],
            )?;
            Some((score, node, title))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.to_lowercase().cmp(&b.2.to_lowercase())));

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
                            DetailField { label: "Type".to_string(), "{contact_type_label(node.contact_type)}" }
                            DetailField { label: "Prefix".to_string(),
                                span { class: "font-mono text-xs", "{node.prefix}" }
                            }
                            DetailField { label: "On Tracker".to_string(),
                                if details_tracked { "Yes" } else { "No" }
                            }
                            DetailField { label: "Last heard".to_string(), "{format_time(node.last_heard)}" }
                            DetailField { label: "Last advert".to_string(), "{format_time(node.last_advert)}" }
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
                                            p { class: "mt-1 whitespace-pre-wrap break-words text-xs", "{msg.text}" }
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
            div { class: "flex flex-col gap-2 sm:flex-row sm:items-center",
                div { class: "min-w-0 flex-1",
                    TextInput {
                        value: query(),
                        placeholder: "Fuzzy search name, prefix, pubkey…".to_string(),
                        kind: "text".to_string(),
                        oninput: move |event: FormEvent| query.set(event.value()),
                    }
                }
                p { class: "text-xs text-muted-foreground", "{ranked.len()} shown" }
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
                            th { class: "px-3 py-2 font-medium", "Actions" }
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
                        for (_score, node, title) in ranked {
                            tr { class: "border-t border-border",
                                td { class: "px-3 py-2",
                                    div { class: "font-medium", "{title}" }
                                    div { class: "font-mono text-xs text-muted-foreground", "{node.prefix}" }
                                }
                                td { class: "px-3 py-2", Badge { tone: node.enrollment.as_str().to_string(), "{node.enrollment.label()}" } }
                                td { class: "px-3 py-2 text-muted-foreground", "{contact_type_label(node.contact_type)}" }
                                td { class: "px-3 py-2", "{format_time(node.last_heard)}" }
                                td { class: "px-3 py-2",
                                    span { class: "inline-flex items-center gap-1.5",
                                        Icon { name: IconName::Locate, size: 14 }
                                        if node.has_gps { "Fix" } else { "None" }
                                    }
                                }
                                td { class: "px-3 py-2",
                                    div { class: "flex flex-wrap justify-end gap-2",
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: {
                                                let pubkey = node.pubkey.clone();
                                                move |_| {
                                                    details_pubkey.set(Some(pubkey.clone()));
                                                }
                                            },
                                            Icon { name: IconName::ChevronRight, size: 14 }
                                            "Details"
                                        }
                                        if node.enrollment != Enrollment::Hq {
                                            Button {
                                                kind: ButtonKind::Ghost,
                                                onclick: {
                                                    let pubkey = node.pubkey.clone();
                                                    let title = title.clone();
                                                    move |_| {
                                                        dm_body.set(String::new());
                                                        note.set(String::new());
                                                        dm_target.set(Some((pubkey.clone(), title.clone())));
                                                    }
                                                },
                                                Icon { name: IconName::Messages, size: 14 }
                                                "Message"
                                            }
                                            Button {
                                                kind: ButtonKind::Ghost,
                                                onclick: {
                                                    let pubkey = node.pubkey.clone();
                                                    let title = title.clone();
                                                    move |_| {
                                                        note.set(String::new());
                                                        invite_target.set(Some((pubkey.clone(), title.clone())));
                                                    }
                                                },
                                                Icon { name: IconName::Key, size: 14 }
                                                "Add channel"
                                            }
                                        }
                                        if node.enrollment == Enrollment::Enrolled {
                                            Button {
                                                kind: ButtonKind::Danger,
                                                onclick: {
                                                    let pubkey = node.pubkey.clone();
                                                    let secrets = secrets.clone();
                                                    move |_| {
                                                        match state().db.revoke(&pubkey, &secrets) {
                                                            Ok(_) => note.set("Revoked and rotated METIS-INFO.".into()),
                                                            Err(err) => note.set(err.to_string()),
                                                        }
                                                    }
                                                },
                                                Icon { name: IconName::Ban, size: 14 }
                                                "Revoke"
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
                    textarea {
                        class: "min-h-28 w-full rounded-md border border-border bg-background px-3 py-2 text-sm text-foreground outline-none",
                        value: dm_body(),
                        maxlength: "160",
                        placeholder: "Write a direct message (max 160 bytes)…",
                        oninput: move |event: FormEvent| dm_body.set(event.value()),
                    }
                    div { class: "flex items-center justify-between gap-2",
                        EmojiPickerButton {
                            on_pick: move |emoji: String| {
                                dm_body.set(append_emoji(&dm_body(), &emoji));
                            },
                        }
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
                                if body.len() > 160 {
                                    note.set("Mesh text is limited to 160 bytes.".into());
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
                open: invite_target().is_some(),
                title: invite_target()
                    .as_ref()
                    .map(|(_, name)| format!("Add to private channel · {name}"))
                    .unwrap_or_else(|| "Add to private channel".into()),
                on_close: move |_| invite_target.set(None),
                div { class: "flex flex-col gap-3",
                    p { class: "text-sm text-muted-foreground",
                        "Sends the channel secret by direct message, the same way signup approval does for METIS-INFO."
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

fn fuzzy_score(query: &str, fields: &[&str]) -> Option<i32> {
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
            PageHeading { icon: IconName::Inbox, title: "Signup inbox".to_string(),
                "A direct message of signup asks to join. Approval sends the METIS-INFO channel secret once. NWS alerts wait here when auto-relay is off."
            }
            if pending.is_empty() {
                Card { title: "Waiting", icon: Some(IconName::Inbox),
                    p { class: "inline-flex items-center gap-2 text-sm text-muted-foreground",
                        Icon { name: IconName::Check, size: 16 }
                        "No pending signup requests."
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

#[component]
fn MessagesPage() -> Element {
    // Select by name, not slot: radio sync remaps slots and used to make a
    // click on Public open METIS-INFO when both rows shared slot 1.
    let mut selected_channel = use_signal(|| None::<String>);
    let mut draft = use_signal(String::new);
    let mut create_name = use_signal(String::new);
    let mut create_visibility = use_signal(|| "private".to_string());
    let mut note = use_signal(String::new);
    let sending = use_signal(|| false);
    let mut reply_target = use_signal(|| None::<MessageReplyTarget>);
    let mut refresh = use_signal(|| 0u64);
    let mut refresh_started = use_signal(|| false);
    let mut last_send_seq = use_signal(|| 0u64);
    let mut read_session = use_signal(|| None::<ChannelReadSession>);
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
                tokio::time::sleep(std::time::Duration::from_millis(750)).await;
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
    // Only enable Send once the live session loop is up (sync_phase Ready).
    // Mid-sync used to report state=connected while the command lock was held,
    // so Send did nothing.
    let linked = radio.state == "connected"
        && matches!(
            radio.sync_phase,
            crate::radio::SyncPhase::Ready | crate::radio::SyncPhase::Idle
        );

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
        let messages = state()
            .db
            .list_channel_messages(slot, 400)
            .unwrap_or_default();
        let emergency = channel.role == "emergency";
        let latest_id = messages.last().map(|message| message.id);
        let needs_session = read_session()
            .as_ref()
            .map(|session| session.slot != slot || session.name != selected_name)
            .unwrap_or(true);
        if needs_session {
            let anchor = state()
                .db
                .channel_last_read(slot)
                .ok()
                .flatten()
                .filter(|id| messages.iter().any(|message| message.id == *id));
            let new_count = match anchor {
                Some(id) => messages.iter().filter(|message| message.id > id).count(),
                None => 0,
            };
            let show_banner = anchor.is_some() && new_count > 0;
            if !show_banner {
                if let Some(id) = latest_id {
                    let _ = state().db.set_channel_last_read(slot, id);
                }
            }
            read_session.set(Some(ChannelReadSession {
                name: selected_name.clone(),
                slot,
                anchor_id: anchor,
                show_banner,
                new_count,
                scrolled: false,
            }));
            let scroll_anchor = anchor;
            let scroll_to_bottom = !show_banner;
            spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(60)).await;
                if scroll_to_bottom {
                    document::eval(
                        r#"const scroller = document.getElementById('channel-scroller');
                           if (scroller) { scroller.scrollTop = scroller.scrollHeight; }"#,
                    );
                } else if let Some(id) = scroll_anchor {
                    document::eval(&format!(
                        r#"const el = document.getElementById('msg-anchor-{id}');
                           if (el) {{ el.scrollIntoView({{ block: 'center', behavior: 'instant' }}); }}"#
                    ));
                }
                if let Some(mut session) = read_session() {
                    session.scrolled = true;
                    read_session.set(Some(session));
                }
                // If unread already fits on screen, clear the banner without waiting for scroll.
                tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                if channel_unread_is_caught_up().await {
                    dismiss_channel_new_banner(read_session);
                }
            });
        } else if let Some(session) = read_session() {
            // While caught up in-channel, advance last-read with live traffic.
            if !session.show_banner {
                if let Some(id) = latest_id {
                    let _ = state().db.set_channel_last_read(slot, id);
                }
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
        return rsx! {
            div { class: "mx-auto flex h-[calc(100vh-8rem)] max-w-4xl flex-col gap-3",
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
                    div { class: "flex flex-wrap items-center gap-2",
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
                        class: "min-h-0 flex-1 space-y-3 overflow-auto p-4",
                        onscroll: move |_| {
                            if !read_session().as_ref().is_some_and(|session| session.show_banner) {
                                return;
                            }
                            spawn(async move {
                                if channel_unread_is_caught_up().await {
                                    dismiss_channel_new_banner(read_session);
                                }
                            });
                        },
                        if messages.is_empty() {
                            p { class: "text-sm text-muted-foreground", "No messages in this channel yet." }
                        }
                        for message in messages {
                            {
                                let outgoing = message.direction == "tx";
                                let show_new_banner = session.show_banner
                                    && session.anchor_id == Some(message.id);
                                let new_count = session.new_count;
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
                                rsx! {
                                    div {
                                        id: "msg-anchor-{message.id}",
                                        key: "{message.id}",
                                        class: if outgoing {
                                            "group relative ml-auto flex max-w-[85%] flex-col items-end gap-1"
                                        } else {
                                            "group relative mr-auto flex max-w-[85%] flex-col items-start gap-1"
                                        },
                                        div {
                                            class: if outgoing {
                                                "min-w-[14rem] rounded-2xl rounded-br-md border border-sky-500/30 bg-[#16324f] px-3 py-2.5 text-sm text-sky-50"
                                            } else if emergency {
                                                "min-w-[14rem] rounded-2xl rounded-bl-md border border-signal/30 bg-background/80 px-3 py-2.5 text-sm"
                                            } else {
                                                "min-w-[14rem] rounded-2xl rounded-bl-md bg-muted px-3 py-2.5 text-sm"
                                            },
                                            div { class: "mb-2 flex items-start justify-between gap-2",
                                                span {
                                                    class: if outgoing {
                                                        "inline-flex max-w-[60%] items-center gap-1 rounded-full bg-sky-400/15 px-2 py-0.5 text-[0.7rem] font-medium text-sky-100"
                                                    } else {
                                                        "inline-flex max-w-[60%] items-center gap-1 rounded-full bg-background/70 px-2 py-0.5 text-[0.7rem] font-medium text-foreground"
                                                    },
                                                    Icon { name: IconName::Radio, size: 11 }
                                                    span { class: "truncate", "{device_name}" }
                                                }
                                                span {
                                                    class: if outgoing {
                                                        "inline-flex shrink-0 items-center gap-1 rounded-full bg-sky-400/10 px-2 py-0.5 text-[0.65rem] text-sky-200/90"
                                                    } else {
                                                        "inline-flex shrink-0 items-center gap-1 rounded-full bg-background/50 px-2 py-0.5 text-[0.65rem] text-muted-foreground"
                                                    },
                                                    Icon { name: IconName::Clock, size: 11 }
                                                    "{format_time(Some(message.received_at))}"
                                                }
                                            }
                                            MessageBody {
                                                text: bubble_text.clone(),
                                                outgoing,
                                            }
                                            div { class: "mt-2 flex justify-start",
                                                if outgoing {
                                                    {
                                                        let (label, icon, tone) = match message.delivery.as_str() {
                                                            "queued" => ("Queued", IconName::Clock, "bg-sky-400/15 text-sky-100"),
                                                            "sending" => ("Sending…", IconName::Clock, "bg-sky-400/15 text-sky-100"),
                                                            "failed" => ("Not sent", IconName::Ban, "bg-red-500/20 text-red-200"),
                                                            _ => ("Sent", IconName::Check, "bg-sky-400/15 text-sky-100"),
                                                        };
                                                        rsx! {
                                                            span {
                                                                class: "inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[0.65rem] {tone}",
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
                                                    span {
                                                        class: "inline-flex items-center gap-1 rounded-full bg-background/60 px-2 py-0.5 text-[0.65rem] text-muted-foreground",
                                                        Icon { name: IconName::Signal, size: 11 }
                                                        if let Some(hops) = message.hop_count {
                                                            "{hops} hops"
                                                        } else {
                                                            "hops —"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        div {
                                            class: if outgoing {
                                                "pointer-events-none absolute -top-3 right-1 z-10 opacity-0 transition group-hover:pointer-events-auto group-hover:opacity-100 group-focus-within:pointer-events-auto group-focus-within:opacity-100"
                                            } else {
                                                "pointer-events-none absolute -top-3 left-1 z-10 opacity-0 transition group-hover:pointer-events-auto group-hover:opacity-100 group-focus-within:pointer-events-auto group-focus-within:opacity-100"
                                            },
                                            div { class: "flex items-center gap-0.5 rounded-md border border-border bg-card/95 p-0.5 shadow-lg backdrop-blur-sm",
                                                button {
                                                    class: "inline-flex h-7 items-center gap-1 rounded px-2 text-[0.7rem] text-foreground hover:bg-muted cursor-pointer",
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
                                                    class: "inline-flex h-7 items-center gap-1 rounded px-2 text-[0.7rem] text-foreground hover:bg-muted cursor-pointer",
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
                                                    class: "inline-flex h-7 items-center gap-1 rounded px-2 text-[0.7rem] text-foreground hover:bg-muted cursor-pointer",
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
                            EmojiPickerButton {
                                disabled: !linked,
                                on_pick: move |emoji: String| {
                                    draft.set(append_emoji(&draft(), &emoji));
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
                                maxlength: "160",
                                oninput: move |event: FormEvent| draft.set(event.value()),
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
                        if !note().is_empty() {
                            p {
                                class: if note().starts_with("Send failed") || note().starts_with("Not sent") {
                                    "mt-2 text-xs text-red-300"
                                } else if note() == "Sending…" {
                                    "mt-2 text-xs text-muted-foreground"
                                } else {
                                    "mt-2 text-xs text-sky-200"
                                },
                                "{note}"
                            }
                        }
                    }
                }
            }
        };
    }

    rsx! {
        div { class: "mx-auto flex max-w-6xl flex-col gap-4",
            PageHeading { icon: IconName::Messages, title: "Channels".to_string(),
                "Public and private channels on the headquarters companion. Secrets are never shown here."
            }
            Card { title: "Default keep".to_string(), icon: Some(IconName::Clock),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "System default for channels and DMs. Each channel can override this on its card or while open. Roster, secrets, and audit stay."
                }
                div { class: "flex flex-wrap gap-2",
                    RetentionChoice { label: "1 hour", value: "1h", current: message_keep(), onpick: move |value: String| {
                        let _ = state().db.set_message_keep(&value);
                        message_keep.set(value);
                        let _ = state().db.apply_message_keep();
                        keep_applied_at.set(crate::db::unix_now());
                    } }
                    RetentionChoice { label: "6 hours", value: "6h", current: message_keep(), onpick: move |value: String| {
                        let _ = state().db.set_message_keep(&value);
                        message_keep.set(value);
                        let _ = state().db.apply_message_keep();
                        keep_applied_at.set(crate::db::unix_now());
                    } }
                    RetentionChoice { label: "24 hours", value: "24h", current: message_keep(), onpick: move |value: String| {
                        let _ = state().db.set_message_keep(&value);
                        message_keep.set(value);
                        let _ = state().db.apply_message_keep();
                        keep_applied_at.set(crate::db::unix_now());
                    } }
                    RetentionChoice { label: "1 week", value: "7d", current: message_keep(), onpick: move |value: String| {
                        let _ = state().db.set_message_keep(&value);
                        message_keep.set(value);
                        let _ = state().db.apply_message_keep();
                        keep_applied_at.set(crate::db::unix_now());
                    } }
                    RetentionChoice { label: "30 days", value: "30d", current: message_keep(), onpick: move |value: String| {
                        let _ = state().db.set_message_keep(&value);
                        message_keep.set(value);
                        let _ = state().db.apply_message_keep();
                        keep_applied_at.set(crate::db::unix_now());
                    } }
                    RetentionChoice { label: "Forever", value: "forever", current: message_keep(), onpick: move |value: String| {
                        let _ = state().db.set_message_keep(&value);
                        message_keep.set(value);
                    } }
                }
            }
            Card { title: "Create channel".to_string(), icon: Some(IconName::Messages),
                div { class: "flex flex-col gap-3 sm:flex-row sm:items-end",
                    div { class: "min-w-0 flex-1",
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
                    div { class: "w-full sm:w-40",
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
                    Button {
                        onclick: move |_| {
                            match state().db.create_channel(&create_name(), &create_visibility(), &state().secrets) {
                                Ok(row) => {
                                    create_name.set(String::new());
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
            }
            div { class: "grid grid-cols-[repeat(auto-fill,minmax(16rem,1fr))] gap-3",
                for channel in channels {
                    {
                        let name = channel.name.clone();
                        let is_public_default = channel.name.eq_ignore_ascii_case("Public");
                        let is_metis_info =
                            channel.name == crate::secrets::CHANNEL_NAME;
                        let can_toggle_visibility = !is_public_default && !is_metis_info;
                        let can_delete = !is_public_default && !is_metis_info;
                        let emergency = channel.role == "emergency";
                        rsx! {
                            div {
                                key: "{channel.name}-{channel.slot}",
                                class: if emergency {
                                    "flex flex-col rounded-lg border border-signal/40 bg-accent/50 p-4"
                                } else {
                                    "flex flex-col rounded-lg border border-border bg-card p-4"
                                },
                                button {
                                    class: "flex flex-1 flex-col items-start gap-3 text-left cursor-pointer",
                                    onclick: move |_| {
                                        note.set(String::new());
                                        selected_channel.set(Some(name.clone()));
                                    },
                                    div { class: "flex w-full items-start justify-between gap-2",
                                        div { class: "flex min-w-0 items-center gap-2",
                                            span {
                                                class: if emergency { "text-signal" } else { "text-muted-foreground" },
                                                Icon {
                                                    name: if emergency { IconName::Radio } else { IconName::Messages },
                                                    size: 18,
                                                }
                                            }
                                            p { class: "truncate font-semibold", "{channel.name}" }
                                            if channel.unread_count > 0 {
                                                span {
                                                    class: "channel-unread-badge",
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
                                    if emergency {
                                        Badge { tone: "signal".to_string(), "HQ emergency" }
                                    }
                                    p { class: "line-clamp-2 text-sm text-muted-foreground",
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
    let result = document::eval(
        r#"return (() => {
          const scroller = document.getElementById('channel-scroller');
          if (!scroller) return false;
          const remaining =
            scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
          if (remaining < 56) return true;
          const marker = document.getElementById('new-messages-marker');
          if (!marker) return false;
          const s = scroller.getBoundingClientRect();
          const m = marker.getBoundingClientRect();
          // Cleared once the divider has moved into / past the upper third.
          return m.bottom < s.top + s.height * 0.35;
        })();"#,
    )
    .await;
    result.ok().and_then(|value| value.as_bool()) == Some(true)
}

fn dismiss_channel_new_banner(mut read_session: Signal<Option<ChannelReadSession>>) {
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
    rsx! {
        p { class: "whitespace-pre-wrap break-words",
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
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
enum TextPart {
    Text(String),
    Mention(String),
    Link(String),
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

fn linkify_text(input: &str) -> Vec<TextPart> {
    let mut parts = Vec::new();
    let mut rest = input;
    while let Some(start) = rest.find("http://").or_else(|| rest.find("https://")) {
        if start > 0 {
            parts.push(TextPart::Text(rest[..start].to_string()));
        }
        let after = &rest[start..];
        let end = after
            .find(|ch: char| ch.is_whitespace() || matches!(ch, ')' | ']' | '"' | '\''))
            .unwrap_or(after.len());
        let mut url = after[..end].to_string();
        while url.ends_with(['.', ',', ';', ':', '!', '?']) {
            url.pop();
        }
        if url.is_empty() {
            parts.push(TextPart::Text(after[..end].to_string()));
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    Hq,
    Device,
    Diagnostics,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeviceTab {
    Identity,
    Radio,
    Network,
    Advanced,
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
fn SettingsPage(radio: RadioStatus, on_request_pin: EventHandler<String>) -> Element {
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
    let mut hq_name = use_signal(|| saved_hq);
    let mut time_format = use_signal(|| saved_time);
    let mut date_format = use_signal(|| saved_date);
    let mut message_keep = use_signal(|| {
        state()
            .db
            .message_keep()
            .unwrap_or_else(|_| "forever".into())
    });
    let mut regions_tick = use_signal(|| 0u32);
    let mut confirm_purge = use_signal(|| false);
    let mut purge_note = use_signal(String::new);
    let mut device_query = use_signal(String::new);
    let mut lat_draft = use_signal(String::new);
    let mut lon_draft = use_signal(String::new);
    let mut tx_draft = use_signal(String::new);
    let mut freq_draft = use_signal(String::new);
    let mut bw_draft = use_signal(String::new);
    let mut sf_draft = use_signal(String::new);
    let mut cr_draft = use_signal(String::new);
    let mut repeat_draft = use_signal(|| false);
    let mut scope_draft = use_signal(String::new);
    let mut preset_open = use_signal(|| false);
    let mut preset_query = use_signal(String::new);
    let mut settings_tab = use_signal(|| SettingsTab::Hq);
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
    let mut svc_alerts_channel = use_signal(|| {
        state()
            .db
            .setting("svc_alerts_channel")
            .ok()
            .flatten()
            .unwrap_or_else(|| "METIS-INFO".into())
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

    let _log_refresh = log_tick();
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
    let saved_radios = state().db.list_saved_radios().unwrap_or_default();
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
    let scanning = radio.state == "scanning" || radio.state == "pairing";
    let busy = matches!(
        radio.state.as_str(),
        "connecting" | "reconnecting" | "pairing" | "scanning" | "disconnecting"
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
                    class: if settings_tab() == SettingsTab::Diagnostics { "settings-tab settings-tab-active" } else { "settings-tab" },
                    onclick: move |_| settings_tab.set(SettingsTab::Diagnostics),
                    Icon { name: IconName::Messages, size: 14 }
                    "Diagnostics"
                }
            }
            if settings_tab() == SettingsTab::Hq {
            Card { title: "Saved companions", icon: Some(IconName::Radio),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Radios you have already paired stay here. Connect reconnects without asking for the PIN again."
                }
                if saved_radios.is_empty() {
                    p { class: "text-sm text-muted-foreground",
                        "None yet. Scan below and use Pair on a MeshCore radio once."
                    }
                } else {
                    div { class: "flex flex-col gap-2",
                        for row in saved_radios {
                            div {
                                key: "{row.address}",
                                class: "flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border bg-background px-3 py-3",
                                div { class: "min-w-0",
                                    p { class: "truncate font-medium",
                                        "{companion_display_name(&row, &radio.devices)}"
                                    }
                                    p { class: "font-mono text-xs text-muted-foreground", "{row.address}" }
                                    if !row.last_error.is_empty() {
                                        p { class: "mt-1 text-xs text-amber-200", "{row.last_error}" }
                                    }
                                }
                                div { class: "flex gap-2",
                                    if radio.state == "connected"
                                        && (radio.connected_name == row.address || radio.connected_name == row.label)
                                    {
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: move |_| state().radio.disconnect(),
                                            Icon { name: IconName::Unlink, size: 14 }
                                            "Disconnect"
                                        }
                                    } else {
                                        Button {
                                            disabled: busy,
                                            onclick: {
                                                let address = row.address.clone();
                                                move |_| {
                                                    radio_name.set(address.clone());
                                                    state().radio.reconnect(address.clone());
                                                }
                                            },
                                            Icon { name: IconName::Bluetooth, size: 14 }
                                            "Connect"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Card { title: "Companion radio", icon: Some(IconName::Bluetooth),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Scan for nearby Bluetooth devices. Use Connect for a radio already on the saved list. Use Pair only for a new radio; that asks for the six-digit PIN. Disconnect the phone app first."
                }
                div { class: "mb-3 flex gap-2",
                    TextInput {
                        value: radio_name(),
                        placeholder: "Name or AA:BB:CC:DD:EE:FF",
                        oninput: move |event: FormEvent| radio_name.set(event.value()),
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
                        Icon { name: IconName::Scan, size: 14 }
                        if scanning { "Scanning" } else { "Scan" }
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| state().radio.disconnect(),
                        Icon { name: IconName::Unlink, size: 14 }
                        "Disconnect"
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
                                placeholder: "Filter name or address",
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
            }
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
            Card { title: "Info services", icon: Some(IconName::Radio),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Bot home is the private METIS-INFO channel (enrolled secret only). Send #weather [zip], #quake, #amber, or #help there. Add + for detail (#weather+ / #amber+) — multi-frame replies. Quake/NWS polls stay silent when nothing new is found. Alerts are labeled with source and time — not an official agency feed."
                }
                div { class: "mb-3 grid gap-3 sm:grid-cols-2",
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
                div { class: "mb-3 flex flex-wrap gap-2",
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
                div { class: "mb-3 flex flex-col gap-2",
                    label { class: "flex items-center gap-2 text-sm",
                        input {
                            r#type: "checkbox",
                            checked: svc_weather(),
                            oninput: move |_| {
                                let next = !svc_weather();
                                svc_weather.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_weather_enabled", next);
                            }
                        }
                        "Weather on METIS-INFO (#weather / #weather+)"
                    }
                    div { class: "ml-6 w-40",
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
                    label { class: "ml-6 flex items-center gap-2 text-sm text-muted-foreground",
                        input {
                            r#type: "checkbox",
                            checked: svc_weather_hourly(),
                            disabled: !svc_weather(),
                            oninput: move |_| {
                                let next = !svc_weather_hourly();
                                svc_weather_hourly.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_weather_hourly", next);
                            }
                        }
                        "Auto-post weather for ZIP list (below)"
                    }
                    div { class: "ml-6 flex flex-col gap-2",
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
                            span { class: "text-xs text-muted-foreground", "minutes (one ZIP per minute when due)" }
                        }
                    }
                    label { class: "flex items-center gap-2 text-sm",
                        input {
                            r#type: "checkbox",
                            checked: svc_nws(),
                            oninput: move |_| {
                                let next = !svc_nws();
                                svc_nws.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_nws_enabled", next);
                            }
                        }
                        "Poll NWS alerts near site pin (US) — silent if none new"
                    }
                    label { class: "ml-6 flex items-center gap-2 text-sm text-muted-foreground",
                        input {
                            r#type: "checkbox",
                            checked: svc_nws_auto(),
                            disabled: !svc_nws(),
                            oninput: move |_| {
                                let next = !svc_nws_auto();
                                svc_nws_auto.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_nws_auto_relay", next);
                            }
                        }
                        "Auto-relay NWS to channel (otherwise Inbox confirms)"
                    }
                    label { class: "flex items-center gap-2 text-sm",
                        input {
                            r#type: "checkbox",
                            checked: svc_quake(),
                            oninput: move |_| {
                                let next = !svc_quake();
                                svc_quake.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_quake_enabled", next);
                            }
                        }
                        "Poll USGS quakes in HQ box — post only when a new one is found"
                    }
                    label { class: "flex items-center gap-2 text-sm",
                        input {
                            r#type: "checkbox",
                            checked: svc_amber(),
                            oninput: move |_| {
                                let next = !svc_amber();
                                svc_amber.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_amber_enabled", next);
                            }
                        }
                        "Amber alerts (NWS CAP CAE) for state — #amber"
                    }
                    label { class: "ml-6 flex items-center gap-2 text-sm text-muted-foreground",
                        input {
                            r#type: "checkbox",
                            checked: svc_amber_auto(),
                            disabled: !svc_amber(),
                            oninput: move |_| {
                                let next = !svc_amber_auto();
                                svc_amber_auto.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_amber_auto_relay", next);
                            }
                        }
                        "Auto-relay Amber to channel (otherwise Inbox)"
                    }
                    label { class: "flex items-center gap-2 text-sm",
                        input {
                            r#type: "checkbox",
                            checked: svc_emerg(),
                            oninput: move |_| {
                                let next = !svc_emerg();
                                svc_emerg.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_emerg_enabled", next);
                            }
                        }
                        "Civil emergencies (evac, hazmat, LE, blue…) — #alert"
                    }
                    label { class: "ml-6 flex items-center gap-2 text-sm text-muted-foreground",
                        input {
                            r#type: "checkbox",
                            checked: svc_emerg_auto(),
                            disabled: !svc_emerg(),
                            oninput: move |_| {
                                let next = !svc_emerg_auto();
                                svc_emerg_auto.set(next);
                                crate::services::set_setting_bool(&state().db, "svc_emerg_auto_relay", next);
                            }
                        }
                        "Auto-relay civil emergencies to channel (otherwise Inbox)"
                    }
                    p { class: "ml-6 text-xs text-muted-foreground",
                        "Sourced from NWS CAP / IPAWS-style codes — labeled ALERT/AMBER with time. Not a law-enforcement feed."
                    }
                }
                div { class: "grid gap-3 sm:grid-cols-2",
                    div {
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
                    div {
                        label { class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground", "Alerts channel" }
                        TextInput {
                            value: svc_alerts_channel(),
                            placeholder: "METIS-INFO",
                            oninput: move |event: FormEvent| {
                                let value = event.value();
                                svc_alerts_channel.set(value.clone());
                                let _ = state().db.set_setting("svc_alerts_channel", value.trim());
                            },
                        }
                    }
                    div {
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
            Card { title: "Channel secret", icon: Some(IconName::Key),
                p { class: "text-sm text-muted-foreground",
                    "Storage: {secret_label}. The secret is sent only after approval, and the message list shows [secret sent]. Revoke on the roster rotates it."
                }
            }
            Card { title: "History", icon: Some(IconName::Clock),
                p { class: "mb-3 text-sm text-muted-foreground",
                    "Default keep for channels and DMs (Channels page can override per channel). Messages auto-trim; use Purge to also clear old routes, locations, and telemetry. Roster, secrets, and audit stay."
                }
                div { class: "mb-3 flex flex-wrap gap-2",
                    RetentionChoice { label: "1 hour", value: "1h", current: message_keep(), onpick: move |value: String| {
                        let _ = state().db.set_message_keep(&value);
                        message_keep.set(value);
                        let _ = state().db.apply_message_keep();
                    } }
                    RetentionChoice { label: "6 hours", value: "6h", current: message_keep(), onpick: move |value: String| {
                        let _ = state().db.set_message_keep(&value);
                        message_keep.set(value);
                        let _ = state().db.apply_message_keep();
                    } }
                    RetentionChoice { label: "24 hours", value: "24h", current: message_keep(), onpick: move |value: String| {
                        let _ = state().db.set_message_keep(&value);
                        message_keep.set(value);
                        let _ = state().db.apply_message_keep();
                    } }
                    RetentionChoice { label: "1 week", value: "7d", current: message_keep(), onpick: move |value: String| {
                        let _ = state().db.set_message_keep(&value);
                        message_keep.set(value);
                        let _ = state().db.apply_message_keep();
                    } }
                    RetentionChoice { label: "30 days", value: "30d", current: message_keep(), onpick: move |value: String| {
                        let _ = state().db.set_message_keep(&value);
                        message_keep.set(value);
                        let _ = state().db.apply_message_keep();
                    } }
                    RetentionChoice { label: "Forever", value: "forever", current: message_keep(), onpick: move |value: String| {
                        let _ = state().db.set_message_keep(&value);
                        message_keep.set(value);
                    } }
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
            {
                let _ = regions_tick();
                let regions = crate::map_tiles::list_installed_regions(&state().db);
                let active = crate::map_tiles::active_region_id(&state().db).unwrap_or_default();
                let dl = crate::map_tiles::download_status();
                rsx! {
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
                            "Identity"
                        }
                        button {
                            class: if device_tab() == DeviceTab::Radio { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                            onclick: move |_| device_tab.set(DeviceTab::Radio),
                            "Radio"
                        }
                        button {
                            class: if device_tab() == DeviceTab::Network { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                            onclick: move |_| device_tab.set(DeviceTab::Network),
                            "Network"
                        }
                        button {
                            class: if device_tab() == DeviceTab::Advanced { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                            onclick: move |_| device_tab.set(DeviceTab::Advanced),
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
                                "Flood region scope and path-hash size for this mesh."
                            }
                            p { class: "mb-2 text-xs text-muted-foreground",
                                if let Some(scope) = radio.flood_scope.as_ref() {
                                    "Current flood scope: {scope}"
                                } else {
                                    "Current flood scope: not set this session"
                                }
                            }
                            div { class: "mb-3 flex flex-col gap-2 sm:flex-row sm:items-end",
                                div { class: "min-w-0 flex-1",
                                    label { class: "mb-1 block text-xs text-muted-foreground", "Default region scope" }
                                    TextInput {
                                        value: scope_draft(),
                                        placeholder: "Region code (blank clears)",
                                        oninput: move |event: FormEvent| scope_draft.set(event.value()),
                                    }
                                }
                                Button {
                                    onclick: move |_| {
                                        let raw = scope_draft().trim().to_string();
                                        if raw.is_empty() {
                                            state().radio.set_flood_scope(None);
                                        } else {
                                            state().radio.set_flood_scope(Some(raw));
                                        }
                                    },
                                    Icon { name: IconName::Check, size: 14 }
                                    "Apply scope"
                                }
                                Button {
                                    kind: ButtonKind::Ghost,
                                    onclick: move |_| {
                                        scope_draft.set(String::new());
                                        state().radio.set_flood_scope(None);
                                    },
                                    "Clear"
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
                                        kind: ButtonKind::Ghost,
                                        onclick: move |_| apply_path_hash_mode_ui(0),
                                        "1-byte"
                                    }
                                    Button {
                                        kind: ButtonKind::Ghost,
                                        onclick: move |_| apply_path_hash_mode_ui(1),
                                        "2-byte"
                                    }
                                    Button {
                                        kind: ButtonKind::Ghost,
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

            Dialog {
                open: confirm_purge(),
                title: "Purge history".to_string(),
                on_close: move |_| confirm_purge.set(false),
                if let Some((cutoff, preview)) = preview {
                    p { class: "mb-3 text-sm text-muted-foreground",
                        "This deletes {preview.messages} messages, {preview.paths} routes, {preview.positions} location trails, and {preview.telemetry} telemetry samples older than the retention window. Roster, secrets, and the audit log stay."
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

/// Per-channel keep override (`default` = inherit global `message_keep`).
#[component]
fn ChannelKeepSelect(
    channel_name: String,
    current: String,
    on_done: EventHandler<String>,
) -> Element {
    let select_id = format!("channel-keep-{}", channel_name.replace(' ', "-"));
    rsx! {
        div { class: "flex min-w-0 flex-wrap items-center gap-2",
            label {
                class: "text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground",
                r#for: "{select_id.clone()}",
                "Keep"
            }
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
