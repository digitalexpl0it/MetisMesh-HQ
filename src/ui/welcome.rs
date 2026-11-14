//! First-run welcome wizard: soft deps + optional companion connect.

use dioxus::prelude::*;
use rand::Rng;

use crate::db::SystemChannel;
use crate::deps;
use crate::radio::RadioStatus;
use crate::secrets::{CHANNEL_NAME, NET_CHANNEL_NAME};

use super::icons::{Icon, IconName};
use super::state;
use super::widgets::{
    name_emoji_count, validate_advert_name_draft, ADVERT_NAME_MAX_BYTES, Button, ButtonKind,
    EmojiPickerButton, SelectInput, TextInput, Throbber,
};

const WELCOME_DONE_KEY: &str = "welcome_done";

#[derive(Clone, Copy, PartialEq, Eq)]
enum WelcomeStep {
    Intro,
    System,
    Companion,
    /// Only reached after a successful companion link.
    NameCompanion,
    /// Pick private channel names (defaults get a random 4-digit suffix).
    Channels,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CompanionTab {
    Bluetooth,
    Usb,
    Network,
}

pub fn welcome_is_done() -> bool {
    state()
        .db
        .setting(WELCOME_DONE_KEY)
        .ok()
        .flatten()
        .as_deref()
        == Some("1")
}

pub fn set_welcome_done(done: bool) {
    let value = if done { "1" } else { "" };
    let _ = state().db.set_setting(WELCOME_DONE_KEY, value);
}

/// Force the wizard to show again (About → Run setup again).
pub fn reopen_welcome() {
    set_welcome_done(false);
}

fn looks_like_bt_address(value: &str) -> bool {
    let trimmed = value.trim();
    let parts: Vec<_> = trimmed.split([':', '-']).collect();
    parts.len() == 6
        && parts.iter().all(|part| {
            part.len() == 2 && part.chars().all(|ch| ch.is_ascii_hexdigit())
        })
}

/// True when a scanned BLE row is the radio we are linking / linked to.
fn device_matches_link(address: &str, name: &str, linked_name: &str) -> bool {
    let linked = linked_name.trim();
    if linked.is_empty() {
        return false;
    }
    address.eq_ignore_ascii_case(linked)
        || (!name.trim().is_empty() && name.eq_ignore_ascii_case(linked))
}

/// Advert / display name for the name step (never prefer a raw MAC when a better label exists).
fn welcome_advert_name(radio: &RadioStatus) -> String {
    let connected = radio.connected_name.trim();
    if !connected.is_empty() && !looks_like_bt_address(connected) {
        return connected.to_string();
    }
    if let Ok(Some(hq)) = state().db.setting("hq_name") {
        let hq = hq.trim();
        if !hq.is_empty() && !looks_like_bt_address(hq) {
            return hq.to_string();
        }
    }
    if !connected.is_empty() {
        if let Some(device) = radio
            .devices
            .iter()
            .find(|device| device.address.eq_ignore_ascii_case(connected))
        {
            let name = device.name.trim();
            if !name.is_empty() && name != "Unnamed device" && !looks_like_bt_address(name) {
                return name.to_string();
            }
        }
        // connected_name may already be the GAP/advert label while scan still lists the MAC.
        if let Some(device) = radio
            .devices
            .iter()
            .find(|device| device.name.eq_ignore_ascii_case(connected))
        {
            let name = device.name.trim();
            if !name.is_empty() && name != "Unnamed device" {
                return name.to_string();
            }
        }
    }
    connected.to_string()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WelcomeFinishAction {
    Dismiss,
    OpenAbout,
}

#[derive(Clone, Copy)]
struct WelcomeStripStep {
    step: WelcomeStep,
    icon: IconName,
    label: &'static str,
}

fn welcome_strip_steps(include_name: bool) -> Vec<WelcomeStripStep> {
    let mut steps = vec![
        WelcomeStripStep {
            step: WelcomeStep::Intro,
            icon: IconName::Radio,
            label: "Welcome",
        },
        WelcomeStripStep {
            step: WelcomeStep::System,
            icon: IconName::Settings,
            label: "System",
        },
        WelcomeStripStep {
            step: WelcomeStep::Companion,
            icon: IconName::Bluetooth,
            label: "Companion",
        },
    ];
    if include_name {
        steps.push(WelcomeStripStep {
            step: WelcomeStep::NameCompanion,
            icon: IconName::Emoji,
            label: "Name",
        });
    }
    steps.push(WelcomeStripStep {
        step: WelcomeStep::Channels,
        icon: IconName::Key,
        label: "Channels",
    });
    steps.push(WelcomeStripStep {
        step: WelcomeStep::Done,
        icon: IconName::Check,
        label: "Done",
    });
    steps
}

/// If a name is still the stock default, append `-XXXX` (same suffix for both when needed).
pub(crate) fn with_unique_suffix_if_default(
    info: &str,
    net: &str,
    suffix: u16,
) -> (String, String) {
    let suffix = format!("{suffix:04}");
    let info = info.trim();
    let net = net.trim();
    let info_out = if info.eq_ignore_ascii_case(CHANNEL_NAME) {
        format!("{CHANNEL_NAME}-{suffix}")
    } else if info.is_empty() {
        format!("{CHANNEL_NAME}-{suffix}")
    } else {
        info.to_string()
    };
    let net_out = if net.eq_ignore_ascii_case(NET_CHANNEL_NAME) {
        format!("{NET_CHANNEL_NAME}-{suffix}")
    } else if net.is_empty() {
        format!("{NET_CHANNEL_NAME}-{suffix}")
    } else {
        net.to_string()
    };
    (info_out, net_out)
}

fn welcome_channel_drafts() -> (String, String) {
    let info = state()
        .db
        .info_channel_name()
        .unwrap_or_else(|_| CHANNEL_NAME.into());
    let net = state()
        .db
        .net_channel_name()
        .unwrap_or_else(|_| NET_CHANNEL_NAME.into());
    let suffix = rand::thread_rng().gen_range(0..10_000u16);
    with_unique_suffix_if_default(&info, &net, suffix)
}

fn save_welcome_channels(info: &str, net: &str) -> Result<(), String> {
    let radio_connected = state().radio.snapshot().state == "connected";
    let (info_name, info_slot) = state()
        .db
        .rename_system_channel(SystemChannel::Info, info, &state().secrets)
        .map_err(|e| e.to_string())?;
    if radio_connected {
        state()
            .radio
            .push_channel(info_slot as u8, info_name.clone());
    }
    let (net_name, net_slot) = state()
        .db
        .rename_system_channel(SystemChannel::Net, net, &state().secrets)
        .map_err(|e| e.to_string())?;
    if radio_connected {
        state().radio.push_channel(net_slot as u8, net_name);
    }
    Ok(())
}

#[component]
fn WelcomeStepStrip(current: WelcomeStep, include_name: bool) -> Element {
    let steps = welcome_strip_steps(include_name);
    rsx! {
        nav {
            class: "welcome-step-strip",
            aria_label: "Setup progress",
            for (idx, item) in steps.iter().enumerate() {
                if idx > 0 {
                    span { class: "welcome-step-arrow", aria_hidden: "true",
                        Icon { name: IconName::ChevronRight, size: 12 }
                    }
                }
                span {
                    class: if current == item.step {
                        "welcome-step-icon welcome-step-icon-active"
                    } else {
                        "welcome-step-icon"
                    },
                    title: "{item.label}",
                    Icon { name: item.icon, size: 16 }
                }
            }
        }
    }
}

#[component]
pub fn WelcomeWizard(
    radio: RadioStatus,
    on_finished: EventHandler<WelcomeFinishAction>,
    on_request_pin: EventHandler<String>,
) -> Element {
    let mut step = use_signal(|| WelcomeStep::Intro);
    let mut soft_tick = use_signal(|| 0u32);
    let mut install_note = use_signal(String::new);
    let mut install_kind = use_signal(|| Option::<deps::SoftDepKind>::None);
    let mut companion_tab = use_signal(|| CompanionTab::Bluetooth);
    let mut ble_target = use_signal(String::new);
    let mut usb_path = use_signal(String::new);
    let mut tcp_host = use_signal(String::new);
    let mut tcp_port = use_signal(|| crate::tcp_radio::DEFAULT_TCP_PORT.to_string());
    let mut name_draft = use_signal(String::new);
    let mut name_note = use_signal(String::new);
    let (initial_info, initial_net) = welcome_channel_drafts();
    let mut channel_info_draft = use_signal(|| initial_info);
    let mut channel_net_draft = use_signal(|| initial_net);
    let mut channel_note = use_signal(String::new);
    let _ = soft_tick();
    let report = deps::soft_status();
    let ble_scanning = radio.state == "scanning" || radio.state == "pairing";
    let usb_scanning = radio.state == "scanning_usb";
    let companion_linking = matches!(
        radio.state.as_str(),
        "pairing" | "connecting" | "reconnecting"
    ) || radio.sync_phase.is_active();
    let companion_linked = radio.state == "connected";
    let detail_lower = radio.detail.to_ascii_lowercase();
    let companion_failed = radio.state == "error"
        || (radio.state == "disconnected"
            && (detail_lower.contains("busy")
                || detail_lower.contains("timeout")
                || detail_lower.contains("failed")
                || detail_lower.contains("not connected")
                || detail_lower.contains("not paired")
                || detail_lower.contains("appstart")));
    let link_key = radio.connected_name.trim().to_string();
    let linked_name = welcome_advert_name(&radio);
    let ble_target_now = ble_target();
    let selected_scan_name = radio
        .devices
        .iter()
        .find(|device| device.address == ble_target_now)
        .map(|device| device.name.clone())
        .unwrap_or_default();
    let selected_is_linked = companion_linked
        && !ble_target_now.is_empty()
        && (device_matches_link(&ble_target_now, &selected_scan_name, &link_key)
            || device_matches_link(&ble_target_now, &selected_scan_name, &linked_name));
    let pair_button_label = if companion_linked && (selected_is_linked || ble_target_now.is_empty())
    {
        "Connected"
    } else if companion_linking {
        "Working…"
    } else if companion_failed {
        "Retry pair"
    } else {
        "Pair with PIN"
    };
    let include_name_strip = companion_linked || step() == WelcomeStep::NameCompanion;
    let emoji_locked = name_emoji_count(&name_draft()) >= 1;
    // Prefer MeshCore companion adverts when the scan can detect them; otherwise
    // keep the full list so a quiet advert still remains selectable.
    let companion_devices: Vec<_> = radio
        .devices
        .iter()
        .filter(|device| device.companion)
        .cloned()
        .collect();
    let ble_list: Vec<_> = if companion_devices.is_empty() {
        radio.devices.clone()
    } else {
        companion_devices
    };
    let ble_list_filtered = !radio.devices.is_empty() && ble_list.len() < radio.devices.len();

    let finish = move |action: WelcomeFinishAction| {
        set_welcome_done(true);
        on_finished.call(action);
    };

    rsx! {
        div { class: "welcome-overlay", role: "dialog", aria_modal: "true", aria_label: "Setup",
            div { class: "welcome-card",
                div { class: "welcome-brand",
                    img {
                        class: "welcome-logo",
                        src: "{crate::brand::default_logo_url()}",
                        alt: "EmergHQ",
                    }
                    span { class: "welcome-app-name", "Metis Mesh HQ" }
                }

                div { class: "welcome-body",
                match step() {
                    WelcomeStep::Intro => rsx! {
                        h1 { class: "welcome-title", "Welcome" }
                        p { class: "welcome-copy",
                            "Local emergency headquarters for MeshCore. No cloud account — this computer talks to your companion radio and keeps the roster, map, and bot running when the internet is down."
                        }
                        p { class: "welcome-copy welcome-copy-muted",
                            "Next we will check system pieces (Bluetooth, USB serial), set your private channel names, and optionally connect a companion."
                        }
                        div { class: "welcome-actions",
                            Button {
                                kind: ButtonKind::Primary,
                                onclick: move |_| step.set(WelcomeStep::System),
                                "Continue"
                            }
                        }
                    },
                    WelcomeStep::System => rsx! {
                        h1 { class: "welcome-title", "System setup" }
                        p { class: "welcome-copy",
                            "These checks help BLE and USB companions work. Optional flasher tools are listed for later."
                        }
                        div { class: "welcome-dep-list",
                            for item in report.items {
                                {
                                    let kind = item.kind;
                                    let label = item.label.clone();
                                    let busy_kind = install_kind();
                                    let this_busy = busy_kind == Some(kind);
                                    let show_install = item.can_install || this_busy;
                                    rsx! {
                                        div { class: "welcome-dep-row",
                                            div { class: "welcome-dep-main",
                                                span { class: if item.ok { "welcome-dep-ok" } else { "welcome-dep-miss" },
                                                    if item.ok {
                                                        "Ready"
                                                    } else if this_busy {
                                                        "Installing"
                                                    } else {
                                                        "Needs attention"
                                                    }
                                                }
                                                div {
                                                    p { class: "welcome-dep-label", "{item.label}" }
                                                    p { class: "welcome-dep-detail", "{item.detail}" }
                                                    if !item.hint.is_empty() {
                                                        p { class: "welcome-dep-hint", "{item.hint}" }
                                                    }
                                                }
                                            }
                                            if show_install {
                                                Button {
                                                    kind: ButtonKind::Ghost,
                                                    disabled: busy_kind.is_some(),
                                                    onclick: move |_| {
                                                        install_kind.set(Some(kind));
                                                        install_note.set(format!("Installing {label}…"));
                                                        spawn(async move {
                                                            let result = tokio::task::spawn_blocking(move || {
                                                                deps::install_soft(kind)
                                                            })
                                                            .await;
                                                            match result {
                                                                Ok(Ok(msg)) => install_note.set(msg),
                                                                Ok(Err(err)) => install_note.set(err),
                                                                Err(err) => install_note
                                                                    .set(format!("Install task failed: {err}")),
                                                            }
                                                            install_kind.set(None);
                                                            deps::invalidate_soft_status();
                                                            soft_tick.with_mut(|n| *n = n.wrapping_add(1));
                                                        });
                                                    },
                                                    if this_busy {
                                                        Throbber { size: 14 }
                                                    }
                                                    if this_busy { "Installing…" } else { "Install" }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if !install_note().is_empty() {
                            p { class: "welcome-note", "{install_note}" }
                        }
                        div { class: "welcome-actions",
                            Button {
                                kind: ButtonKind::Ghost,
                                disabled: install_kind().is_some(),
                                onclick: move |_| {
                                    deps::invalidate_soft_status();
                                    soft_tick.with_mut(|n| *n = n.wrapping_add(1));
                                    install_note.set(String::new());
                                },
                                "Refresh"
                            }
                            Button {
                                kind: ButtonKind::Ghost,
                                disabled: install_kind().is_some(),
                                onclick: move |_| step.set(WelcomeStep::Companion),
                                "Skip"
                            }
                            Button {
                                kind: ButtonKind::Primary,
                                disabled: install_kind().is_some(),
                                onclick: move |_| step.set(WelcomeStep::Companion),
                                "Continue"
                            }
                        }
                    },
                    WelcomeStep::Companion => rsx! {
                        h1 { class: "welcome-title", "Connect a companion" }
                        p { class: "welcome-copy",
                            "Optional. Pair a MeshCore companion now, or skip and use Settings → Connect later."
                        }
                        div { class: "welcome-link-status",
                            if companion_linking {
                                span { class: "welcome-link-row",
                                    Throbber { size: 14 }
                                    span { class: "welcome-note",
                                        if radio.state == "pairing" {
                                            "Pairing… enter the same PIN in the Windows Bluetooth prompt if it appears, then wait for the link."
                                        } else if radio.sync_phase.is_active() {
                                            "Linked — syncing ({radio.sync_label}). Stay here until it finishes."
                                        } else {
                                            "Connecting… {radio.detail}"
                                        }
                                    }
                                }
                            } else if companion_linked {
                                p { class: "welcome-note welcome-note-ok",
                                    span { class: "welcome-inline-check",
                                        Icon { name: IconName::Check, size: 14 }
                                    }
                                    if linked_name.is_empty() {
                                        "Connected. You can continue."
                                    } else {
                                        "Connected to {linked_name}. You can continue."
                                    }
                                }
                            } else if companion_failed {
                                p { class: "welcome-note welcome-note-warn",
                                    "{radio.detail}"
                                }
                            } else {
                                p { class: "welcome-copy-muted",
                                    "Scan, select your companion, then Pair with PIN. The phone MeshCore app must be disconnected first."
                                }
                            }
                        }
                        div { class: "welcome-transport-tabs",
                            button {
                                class: if companion_tab() == CompanionTab::Bluetooth { "welcome-transport active" } else { "welcome-transport" },
                                disabled: companion_linking,
                                onclick: move |_| companion_tab.set(CompanionTab::Bluetooth),
                                "Bluetooth"
                            }
                            button {
                                class: if companion_tab() == CompanionTab::Usb { "welcome-transport active" } else { "welcome-transport" },
                                disabled: companion_linking,
                                onclick: move |_| {
                                    companion_tab.set(CompanionTab::Usb);
                                    state().radio.scan_usb();
                                },
                                "USB"
                            }
                            button {
                                class: if companion_tab() == CompanionTab::Network { "welcome-transport active" } else { "welcome-transport" },
                                disabled: companion_linking,
                                onclick: move |_| companion_tab.set(CompanionTab::Network),
                                "TCP"
                            }
                        }
                        match companion_tab() {
                            CompanionTab::Bluetooth => rsx! {
                                div { class: "welcome-companion-body",
                                    div { class: "welcome-actions",
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            disabled: ble_scanning,
                                            onclick: move |_| state().radio.scan(),
                                            if ble_scanning {
                                                Throbber { size: 14 }
                                            } else {
                                                Icon { name: IconName::Bluetooth, size: 14 }
                                            }
                                            if ble_scanning { "Scanning…" } else { "Scan" }
                                        }
                                    }
                                    if radio.devices.is_empty() {
                                        p { class: "welcome-copy-muted", "No Bluetooth devices listed yet. Scan with the phone app disconnected from the radio." }
                                    } else {
                                        if ble_list_filtered {
                                            p { class: "welcome-copy-muted",
                                                "Showing {ble_list.len()} MeshCore companion(s); other Bluetooth devices are hidden."
                                            }
                                        }
                                        div {
                                            class: "welcome-device-list",
                                            role: "radiogroup",
                                            aria_label: "MeshCore companions",
                                            for device in ble_list.clone() {
                                                {
                                                    let addr = device.address.clone();
                                                    let name = device.name.clone();
                                                    let title = if device.name.trim().is_empty()
                                                        || device.name == "Unnamed device"
                                                    {
                                                        None
                                                    } else {
                                                        Some(device.name.clone())
                                                    };
                                                    let selected = ble_target() == addr;
                                                    let linked = (companion_linked || companion_linking)
                                                        && (device_matches_link(
                                                            &addr, &name, &link_key,
                                                        ) || device_matches_link(
                                                            &addr, &name, &linked_name,
                                                        ));
                                                    let row_busy = companion_linking
                                                        && (selected
                                                            || device_matches_link(
                                                                &addr, &name, &link_key,
                                                            )
                                                            || device_matches_link(
                                                                &addr, &name, &linked_name,
                                                            )
                                                            || (!ble_target_now.is_empty()
                                                                && addr == ble_target_now));
                                                    let checked = selected || (linked && companion_linked);
                                                    let class = if linked && companion_linked {
                                                        "welcome-device welcome-device-linked"
                                                    } else if selected || row_busy {
                                                        "welcome-device active"
                                                    } else {
                                                        "welcome-device"
                                                    };
                                                    rsx! {
                                                        button {
                                                            class: "{class}",
                                                            role: "radio",
                                                            aria_checked: if checked { "true" } else { "false" },
                                                            disabled: companion_linking,
                                                            onclick: move |_| ble_target.set(addr.clone()),
                                                            span { class: "welcome-device-main",
                                                                span { class: "welcome-device-lead",
                                                                    span {
                                                                        class: "welcome-device-check",
                                                                        aria_hidden: "true",
                                                                        if checked {
                                                                            Icon { name: IconName::Check, size: 12 }
                                                                        }
                                                                    }
                                                                    span { class: "welcome-device-copy",
                                                                        if let Some(title) = title {
                                                                            span { class: "welcome-device-label", "{title}" }
                                                                            span { class: "welcome-device-addr", "{device.address}" }
                                                                        } else {
                                                                            span { class: "welcome-device-label", "{device.address}" }
                                                                        }
                                                                    }
                                                                }
                                                                if linked && companion_linked {
                                                                    span { class: "welcome-device-badge welcome-device-badge-ok",
                                                                        Icon { name: IconName::Check, size: 12 }
                                                                        "Connected"
                                                                    }
                                                                } else if row_busy {
                                                                    span { class: "welcome-device-badge",
                                                                        Throbber { size: 12 }
                                                                        if radio.state == "pairing" {
                                                                            "Pairing…"
                                                                        } else {
                                                                            "Connecting…"
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
                                    div { class: "welcome-actions welcome-pair-row",
                                        Button {
                                            kind: if companion_linked && selected_is_linked {
                                                ButtonKind::Ghost
                                            } else {
                                                ButtonKind::Primary
                                            },
                                            disabled: ble_target().trim().is_empty()
                                                || companion_linking
                                                || (companion_linked && selected_is_linked),
                                            onclick: move |_| {
                                                on_request_pin.call(ble_target());
                                            },
                                            if companion_linking {
                                                Throbber { size: 14 }
                                            } else if companion_linked && selected_is_linked {
                                                Icon { name: IconName::Check, size: 14 }
                                            } else {
                                                Icon { name: IconName::Bluetooth, size: 14 }
                                            }
                                            "{pair_button_label}"
                                        }
                                        if companion_linked {
                                            span { class: "welcome-pair-hint welcome-note-ok",
                                                Icon { name: IconName::Check, size: 14 }
                                                "Companion linked"
                                            }
                                        } else if companion_linking {
                                            span { class: "welcome-pair-hint",
                                                "Linking… status updates above"
                                            }
                                        }
                                    }
                                }
                            },
                            CompanionTab::Usb => rsx! {
                                div { class: "welcome-companion-body",
                                    div { class: "welcome-actions",
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
                                    if radio.usb_ports.is_empty() {
                                        p { class: "welcome-copy-muted", "No USB serial ports found. Plug in a USB companion and refresh." }
                                    } else {
                                        SelectInput {
                                            id: "welcome-usb".to_string(),
                                            value: usb_path(),
                                            onchange: move |event: FormEvent| usb_path.set(event.value()),
                                            option {
                                                value: "",
                                                style: "background-color:#101418;color:#f4f4f5;",
                                                "Choose a port…"
                                            }
                                            for port in radio.usb_ports.clone() {
                                                option {
                                                    value: "{port.path}",
                                                    style: "background-color:#101418;color:#f4f4f5;",
                                                    "{port.label} — {port.path}"
                                                }
                                            }
                                        }
                                    }
                                    div { class: "welcome-actions",
                                        Button {
                                            kind: ButtonKind::Primary,
                                            disabled: usb_path().trim().is_empty() || companion_linking,
                                            onclick: move |_| {
                                                state().radio.connect_usb(usb_path());
                                            },
                                            if companion_linking { "Working…" } else { "Connect USB" }
                                        }
                                    }
                                }
                            },
                            CompanionTab::Network => rsx! {
                                div { class: "welcome-companion-body welcome-tcp-grid",
                                    TextInput {
                                        value: tcp_host(),
                                        placeholder: "Host or IP",
                                        oninput: move |event: FormEvent| tcp_host.set(event.value()),
                                    }
                                    TextInput {
                                        value: tcp_port(),
                                        placeholder: "Port",
                                        oninput: move |event: FormEvent| tcp_port.set(event.value()),
                                    }
                                    div { class: "welcome-actions",
                                        Button {
                                            kind: ButtonKind::Primary,
                                            disabled: tcp_host().trim().is_empty() || companion_linking,
                                            onclick: move |_| {
                                                let host = tcp_host().trim().to_string();
                                                let port = tcp_port().trim().to_string();
                                                let addr = if port.is_empty() {
                                                    host
                                                } else {
                                                    format!("{host}:{port}")
                                                };
                                                state().radio.connect_tcp(addr);
                                            },
                                            if companion_linking { "Working…" } else { "Connect TCP" }
                                        }
                                    }
                                }
                            },
                        }
                        div { class: "welcome-actions",
                            Button {
                                kind: ButtonKind::Ghost,
                                disabled: companion_linking,
                                onclick: move |_| step.set(WelcomeStep::System),
                                "Back"
                            }
                            Button {
                                kind: ButtonKind::Ghost,
                                disabled: companion_linking,
                                onclick: move |_| {
                                    channel_note.set(String::new());
                                    step.set(WelcomeStep::Channels);
                                },
                                if companion_linking { "Wait…" } else { "Skip" }
                            }
                            Button {
                                kind: ButtonKind::Primary,
                                disabled: companion_linking,
                                onclick: move |_| {
                                    if companion_linked {
                                        name_draft.set(linked_name.clone());
                                        name_note.set(String::new());
                                        step.set(WelcomeStep::NameCompanion);
                                    } else {
                                        channel_note.set(String::new());
                                        step.set(WelcomeStep::Channels);
                                    }
                                },
                                if companion_linked {
                                    "Continue"
                                } else if companion_linking {
                                    "Connecting…"
                                } else {
                                    "Continue without radio"
                                }
                            }
                        }
                    },
                    WelcomeStep::NameCompanion => rsx! {
                        h1 { class: "welcome-title", "Name this companion" }
                        p { class: "welcome-copy",
                            "Set the advert name shown on the mesh. At most one emoji when renaming here."
                        }
                        p { class: "welcome-copy-muted",
                            if linked_name.is_empty() {
                                "Current name: (none yet)"
                            } else {
                                "Current name: {linked_name}"
                            }
                        }
                        div { class: "welcome-name-row",
                            TextInput {
                                value: name_draft(),
                                placeholder: "Companion name",
                                oninput: move |event: FormEvent| {
                                    name_note.set(String::new());
                                    name_draft.set(event.value());
                                },
                            }
                            EmojiPickerButton {
                                disabled: emoji_locked,
                                align_end: true,
                                // Open upward so the panel clears the step strip.
                                drop_down: false,
                                on_pick: move |emoji: String| {
                                    if name_emoji_count(&name_draft()) >= 1 {
                                        return;
                                    }
                                    let draft = name_draft();
                                    if draft.len().saturating_add(emoji.len()) > ADVERT_NAME_MAX_BYTES
                                    {
                                        name_note.set(
                                            "Name is too long for another character.".into(),
                                        );
                                        return;
                                    }
                                    name_note.set(String::new());
                                    name_draft.set(format!("{draft}{emoji}"));
                                },
                            }
                        }
                        if !name_note().is_empty() {
                            p { class: "welcome-note welcome-note-warn", "{name_note()}" }
                        }
                        div { class: "welcome-actions",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| step.set(WelcomeStep::Companion),
                                "Back"
                            }
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    channel_note.set(String::new());
                                    step.set(WelcomeStep::Channels);
                                },
                                "Keep current name"
                            }
                            Button {
                                kind: ButtonKind::Primary,
                                onclick: move |_| {
                                    match validate_advert_name_draft(&name_draft()) {
                                        Ok(name) => {
                                            name_note.set(String::new());
                                            state().radio.apply_name(name);
                                            channel_note.set(String::new());
                                            step.set(WelcomeStep::Channels);
                                        }
                                        Err(msg) => name_note.set(msg),
                                    }
                                },
                                "Save and continue"
                            }
                        }
                    },
                    WelcomeStep::Channels => rsx! {
                        h1 { class: "welcome-title", "Private channels" }
                        p { class: "welcome-copy",
                            "Stock names "
                            code { "{CHANNEL_NAME}" }
                            " / "
                            code { "{NET_CHANNEL_NAME}" }
                            " are already common on the mesh. Defaults below get a random 4-digit suffix — edit freely. You can rename again later under Settings → Preferences."
                        }
                        div { class: "welcome-channel-grid",
                            div {
                                label { class: "welcome-channel-label", "Bot home (INFO)" }
                                TextInput {
                                    value: channel_info_draft(),
                                    placeholder: CHANNEL_NAME,
                                    oninput: move |event: FormEvent| {
                                        channel_note.set(String::new());
                                        channel_info_draft.set(event.value());
                                    },
                                }
                            }
                            div {
                                label { class: "welcome-channel-label", "Peer net (NET)" }
                                TextInput {
                                    value: channel_net_draft(),
                                    placeholder: NET_CHANNEL_NAME,
                                    oninput: move |event: FormEvent| {
                                        channel_note.set(String::new());
                                        channel_net_draft.set(event.value());
                                    },
                                }
                            }
                        }
                        if !channel_note().is_empty() {
                            p { class: "welcome-note welcome-note-warn", "{channel_note()}" }
                        }
                        div { class: "welcome-actions",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    if companion_linked {
                                        step.set(WelcomeStep::NameCompanion);
                                    } else {
                                        step.set(WelcomeStep::Companion);
                                    }
                                },
                                "Back"
                            }
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    let suffix = rand::thread_rng().gen_range(0..10_000u16);
                                    let (info, net) = with_unique_suffix_if_default(
                                        CHANNEL_NAME,
                                        NET_CHANNEL_NAME,
                                        suffix,
                                    );
                                    channel_info_draft.set(info);
                                    channel_net_draft.set(net);
                                    channel_note.set(String::new());
                                },
                                "New suffix"
                            }
                            Button {
                                kind: ButtonKind::Primary,
                                onclick: move |_| {
                                    match save_welcome_channels(
                                        &channel_info_draft(),
                                        &channel_net_draft(),
                                    ) {
                                        Ok(()) => {
                                            channel_note.set(String::new());
                                            step.set(WelcomeStep::Done);
                                        }
                                        Err(err) => channel_note.set(err),
                                    }
                                },
                                "Save and continue"
                            }
                        }
                    },
                    WelcomeStep::Done => rsx! {
                        h1 { class: "welcome-title", "You're set" }
                        p { class: "welcome-copy",
                            if companion_linked {
                                "Companion is linked. Private channels are saved. Connect or manage radios anytime under Settings → Connect. Open About for version and data folder details."
                            } else {
                                "HQ is ready. Private channels are saved. Connect or manage radios anytime under Settings → Connect. Open About for version and data folder details."
                            }
                        }
                        div { class: "welcome-actions",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| finish(WelcomeFinishAction::OpenAbout),
                                "About"
                            }
                            Button {
                                kind: ButtonKind::Primary,
                                onclick: move |_| finish(WelcomeFinishAction::Dismiss),
                                "Open headquarters"
                            }
                        }
                    },
                }
                }

                WelcomeStepStrip {
                    current: step(),
                    include_name: include_name_strip,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_names_get_shared_suffix() {
        let (info, net) = with_unique_suffix_if_default(CHANNEL_NAME, NET_CHANNEL_NAME, 42);
        assert_eq!(info, "METIS-INFO-0042");
        assert_eq!(net, "METIS-NET-0042");
    }

    #[test]
    fn custom_names_kept() {
        let (info, net) = with_unique_suffix_if_default("HQ-BOT", "HQ-PEERS", 99);
        assert_eq!(info, "HQ-BOT");
        assert_eq!(net, "HQ-PEERS");
    }

    #[test]
    fn only_default_side_gets_suffix() {
        let (info, net) = with_unique_suffix_if_default(CHANNEL_NAME, "CHURCH-NET", 7);
        assert_eq!(info, "METIS-INFO-0007");
        assert_eq!(net, "CHURCH-NET");
    }

    #[test]
    fn empty_treated_as_default() {
        let (info, net) = with_unique_suffix_if_default("", "", 1234);
        assert_eq!(info, "METIS-INFO-1234");
        assert_eq!(net, "METIS-NET-1234");
    }
}
