//! First-run welcome wizard: soft deps + optional companion connect.

use dioxus::prelude::*;

use crate::deps;
use crate::radio::RadioStatus;

use super::icons::{Icon, IconName};
use super::state;
use super::widgets::{Button, ButtonKind, SelectInput, TextInput, Throbber};

const WELCOME_DONE_KEY: &str = "welcome_done";

#[derive(Clone, Copy, PartialEq, Eq)]
enum WelcomeStep {
    Intro,
    System,
    Companion,
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

/// Force the wizard to show again (Settings → About).
pub fn reopen_welcome() {
    set_welcome_done(false);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum WelcomeFinishAction {
    Dismiss,
    OpenAbout,
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
    let _ = soft_tick();
    let report = deps::soft_status();
    let ble_scanning = radio.state == "scanning" || radio.state == "pairing";
    let usb_scanning = radio.state == "scanning_usb";
    let companion_linking = matches!(
        radio.state.as_str(),
        "pairing" | "connecting" | "reconnecting"
    ) || radio.sync_phase.is_active();
    let companion_linked = radio.state == "connected";
    let companion_failed = radio.state == "error"
        || radio.state == "disconnected"
            && radio
                .detail
                .to_ascii_lowercase()
                .contains("busy");

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

                match step() {
                    WelcomeStep::Intro => rsx! {
                        h1 { class: "welcome-title", "Welcome" }
                        p { class: "welcome-copy",
                            "Local emergency headquarters for MeshCore. No cloud account — this computer talks to your companion radio and keeps the roster, map, and bot running when the internet is down."
                        }
                        p { class: "welcome-copy welcome-copy-muted",
                            "Next we will check system pieces (Bluetooth, USB serial) and optionally connect a companion."
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
                                            "Connected — syncing companion ({radio.sync_label}). Stay on this step until ready."
                                        } else {
                                            "Connecting… {radio.detail}"
                                        }
                                    }
                                }
                            } else if companion_linked {
                                p { class: "welcome-note welcome-note-ok",
                                    "Connected — {radio.detail}. You can continue."
                                }
                            } else if companion_failed {
                                p { class: "welcome-note welcome-note-warn",
                                    "{radio.detail}"
                                }
                            } else {
                                p { class: "welcome-note",
                                    "Radio: {radio.state} — {radio.detail}"
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
                                        div { class: "welcome-device-list",
                                            for device in radio.devices.clone() {
                                                {
                                                    let addr = device.address.clone();
                                                    let label = if device.name.trim().is_empty() {
                                                        device.address.clone()
                                                    } else {
                                                        format!("{} ({})", device.name, device.address)
                                                    };
                                                    let selected = ble_target() == addr;
                                                    rsx! {
                                                        button {
                                                            class: if selected { "welcome-device active" } else { "welcome-device" },
                                                            onclick: move |_| ble_target.set(addr.clone()),
                                                            "{label}"
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    div { class: "welcome-actions",
                                        Button {
                                            kind: ButtonKind::Primary,
                                            disabled: ble_target().trim().is_empty() || companion_linking,
                                            onclick: move |_| {
                                                on_request_pin.call(ble_target());
                                            },
                                            if companion_linking {
                                                Throbber { size: 14 }
                                            } else {
                                                Icon { name: IconName::Bluetooth, size: 14 }
                                            }
                                            if companion_linking { "Working…" } else { "Pair with PIN" }
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
                                onclick: move |_| step.set(WelcomeStep::Done),
                                if companion_linking { "Wait…" } else { "Skip" }
                            }
                            Button {
                                kind: ButtonKind::Primary,
                                disabled: companion_linking,
                                onclick: move |_| step.set(WelcomeStep::Done),
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
                    WelcomeStep::Done => rsx! {
                        h1 { class: "welcome-title", "You're set" }
                        p { class: "welcome-copy",
                            if companion_linked {
                                "Companion is linked. Connect or manage radios anytime under Settings → Connect. Open About for version and data folder details."
                            } else {
                                "HQ is ready. Connect or manage radios anytime under Settings → Connect. Open About for version and data folder details."
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
        }
    }
}
