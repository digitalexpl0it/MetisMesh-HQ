//! In-app MeshCore CLI terminal (local companion + remote contacts) + RF RX log.

use dioxus::prelude::*;

use super::icons::{Icon, IconName};
use super::state;
use super::widgets::{Button, ButtonKind, Dialog, PageHeading, SelectInput, TextInput};
use crate::radio::RadioStatus;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CliPageTab {
    Cli,
    RxLog,
}

fn rx_log_clock(at: i64) -> String {
    use chrono::{Local, TimeZone};
    Local
        .timestamp_opt(at, 0)
        .single()
        .map(|t| t.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| at.to_string())
}

/// Firmware role for which CLI vocabulary to offer.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CliMode {
    Companion,
    Repeater,
    Room,
    Sensor,
}

impl CliMode {
    pub fn from_contact_type(contact_type: u8) -> Self {
        match contact_type {
            2 => Self::Repeater,
            3 => Self::Room,
            4 => Self::Sensor,
            // Chat / unknown / enrolled companion → companion CLI.
            _ => Self::Companion,
        }
    }

    pub fn from_infra_kind(kind: &str) -> Self {
        match kind.trim().to_ascii_lowercase().as_str() {
            "room" => Self::Room,
            "sensor" => Self::Sensor,
            _ => Self::Repeater,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Companion => "Companion",
            Self::Repeater => "Repeater",
            Self::Room => "Room / data server",
            Self::Sensor => "Sensor",
        }
    }
}

/// Local companion CLI (firmware v1.18+ / protocol 14+). Not the repeater docs set.
const COMPANION_COMMANDS: &[(&str, &str)] = &[
    ("ver", "Firmware version"),
    ("board", "Board name"),
    ("get name", "Node name"),
    ("get radio", "Freq / BW / SF / CR"),
    ("get freq", "Frequency only"),
    ("get tx", "TX power"),
    ("get af", "Airtime factor"),
    ("get dutycycle", "Duty cycle %"),
    ("get radio.rxgain", "RX boosted gain"),
    ("get rxdelay", "RX delay base"),
    ("get txdelay", "Flood TX delay"),
    ("get direct.txdelay", "Direct TX delay"),
    ("get cad", "Channel activity detect"),
    ("get int.thresh", "Interference threshold"),
    ("get path.hash.mode", "Path hash size"),
    ("get multi.acks", "Multi-ACK"),
    ("get tz.offset", "Timezone offset"),
    ("get wifi.enabled", "WiFi enabled"),
    ("get wifi.ssid", "WiFi SSID"),
    ("get wifi.status", "WiFi status"),
    ("get wifi.ip", "WiFi IP"),
    ("reboot", "Reboot companion"),
];

/// Shared repeater / room / sensor reads from docs.meshcore.io/cli_commands.
const INFRA_COMMON: &[(&str, &str)] = &[
    ("ver", "Firmware version"),
    ("board", "Board name"),
    ("clock", "Current UTC time"),
    ("clock sync", "Sync clock from sender"),
    ("get name", "Node name"),
    ("get public.key", "Public key"),
    ("get radio", "Freq / BW / SF / CR"),
    ("get freq", "Frequency"),
    ("get tx", "TX power"),
    ("get lat", "Latitude"),
    ("get lon", "Longitude"),
    ("advert", "Flood advert"),
    ("advert.zerohop", "Zero-hop advert"),
    ("neighbors", "Nearby neighbors"),
    ("discover.neighbors", "Discover zero-hop neighbors"),
    ("get repeat", "Repeat flag"),
    ("get path.hash.mode", "Path hash size"),
    ("get loop.detect", "Loop detection"),
    ("get txdelay", "Flood TX delay"),
    ("get direct.txdelay", "Direct TX delay"),
    ("get rxdelay", "RX delay"),
    ("get dutycycle", "Duty cycle %"),
    ("get af", "Airtime factor (legacy)"),
    ("get radio.rxgain", "RX boosted gain"),
    ("get cad", "Channel activity detect"),
    ("get int.thresh", "Interference threshold"),
    ("get flood.advert.interval", "Flood advert interval"),
    ("get advert.interval", "Zero-hop advert interval"),
    ("get flood.max", "Flood hop limit"),
    ("get flood.max.unscoped", "Unscoped flood hop limit"),
    ("get flood.max.advert", "Advert flood hop limit"),
    ("region", "Dump regions"),
    ("clear stats", "Clear stats"),
    ("reboot", "Reboot node"),
];

const REPEATER_COMMANDS: &[(&str, &str)] = INFRA_COMMON;

const ROOM_EXTRA: &[(&str, &str)] = &[
    ("get guest.password", "Guest password"),
    ("get allow.read.only", "Read-only room flag"),
    ("get owner.info", "Owner info"),
];

const SENSOR_EXTRA: &[(&str, &str)] = &[
    ("gps", "GPS status"),
    ("gps sync", "Sync clock from GPS"),
    ("gps setloc", "Set lat/lon from GPS"),
    ("gps advert", "GPS advert policy"),
    ("sensor list", "List sensors"),
];

fn room_commands() -> Vec<(&'static str, &'static str)> {
    let mut out = INFRA_COMMON.to_vec();
    out.extend_from_slice(ROOM_EXTRA);
    out
}

fn sensor_commands() -> Vec<(&'static str, &'static str)> {
    let mut out = INFRA_COMMON.to_vec();
    out.extend_from_slice(SENSOR_EXTRA);
    out
}

// Pre-built for Room/Sensor so `commands()` can return a slice without allocating each render.
// Room and Sensor need extras; use lazy static via once_cell or just allocate in the component.
// Simpler: return owned Vec from a helper used once when mode changes.

pub fn commands_for(mode: CliMode) -> Vec<(&'static str, &'static str)> {
    match mode {
        CliMode::Companion => COMPANION_COMMANDS.to_vec(),
        CliMode::Repeater => REPEATER_COMMANDS.to_vec(),
        CliMode::Room => room_commands(),
        CliMode::Sensor => sensor_commands(),
    }
}

/// CLI follow-ups after a binary status refresh (fields not in StatusData).
pub const STATUS_CLI_FOLLOWUP: &[&str] =
    &["get tx", "ver", "clock", "get radio", "neighbors"];

pub fn is_destructive_cli(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    ["erase", "reboot", "poweroff", "shutdown", "factory", "clkreboot"]
        .iter()
        .any(|prefix| lower == *prefix || lower.starts_with(&format!("{prefix} ")))
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

fn contact_type_label(value: u8) -> &'static str {
    match value {
        1 => "Companion",
        2 => "Repeater",
        3 => "Room / data server",
        4 => "Sensor",
        _ => "Node",
    }
}

#[component]
pub fn CliPage(radio: RadioStatus) -> Element {
    let mut page_tab = use_signal(|| CliPageTab::Cli);
    let mut draft = use_signal(String::new);
    let mut target = use_signal(|| "local".to_string());
    let mut preset = use_signal(|| String::new());
    let mut transcript = use_signal(|| {
        vec![
            "MeshCore CLI — command list follows the selected target's role.".to_string(),
            "Companion uses protocol v14+ opcode 66; remote uses TXT_TYPE_CLI_COMMAND.".to_string(),
            "Docs (repeater / room / sensor): https://docs.meshcore.io/cli_commands/".to_string(),
        ]
    });
    let mut confirm_open = use_signal(|| false);
    let mut pending = use_signal(String::new);
    let mut last_seq = use_signal(|| 0u64);
    let mut poll = use_signal(|| 0u32);
    let mut poll_started = use_signal(|| false);
    let mut rx_seq = use_signal(|| 0u64);
    let mut rx_lines = use_signal(Vec::<crate::radio::RxLogLine>::new);
    let mut rx_autoscroll = use_signal(|| true);

    if !poll_started() {
        poll_started.set(true);
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                poll.with_mut(|n| *n = n.wrapping_add(1));
            }
        });
    }

    // Subscribe LogData only while the RX log tab is visible.
    use_effect(move || {
        let want = page_tab() == CliPageTab::RxLog;
        state().radio.set_rx_log_wanted(want);
    });
    use_drop(move || {
        state().radio.set_rx_log_wanted(false);
    });

    let linked = radio.state == "connected";
    let contacts = state()
        .db
        .list_nodes()
        .unwrap_or_default()
        .into_iter()
        .filter(|n| {
            // 2 repeater, 3 room, 4 sensor — also allow chat / enrolled for remote companion CLI.
            matches!(n.contact_type, 1 | 2 | 3 | 4) || n.enrollment.as_str() == "enrolled"
        })
        .collect::<Vec<_>>();

    let mode = if target() == "local" {
        CliMode::Companion
    } else {
        contacts
            .iter()
            .find(|n| n.pubkey == target())
            .map(|n| CliMode::from_contact_type(n.contact_type))
            .unwrap_or(CliMode::Companion)
    };
    let catalog = commands_for(mode);
    let placeholder = catalog
        .first()
        .map(|(cmd, _)| (*cmd).to_string())
        .unwrap_or_else(|| "ver".into());

    // Poll radio status for CLI replies + RX log (avoid mutating signals during render).
    use_effect(move || {
        let _ = poll();
        let snap = state().radio.snapshot();
        if snap.cli_seq != 0 && snap.cli_seq != last_seq() && !snap.cli_line.is_empty() {
            last_seq.set(snap.cli_seq);
            append_transcript(transcript, &snap.cli_line);
        }
        if page_tab() == CliPageTab::RxLog {
            let (seq, lines) = state().radio.rx_log_snapshot();
            if seq != rx_seq() {
                rx_seq.set(seq);
                rx_lines.set(lines);
            }
        }
    });

    use_effect(move || {
        let _ = rx_seq();
        if page_tab() != CliPageTab::RxLog || !rx_autoscroll() {
            return;
        }
        spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            let _ = document::eval(
                r#"const el = document.getElementById('hq-rx-log-transcript');
                   if (el) { el.scrollTop = el.scrollHeight; }"#,
            );
        });
    });

    let send_command = move |text: String| {
        if !linked {
            append_transcript(transcript, "Not connected — pair a companion first.");
            return;
        }
        let contact = if target() == "local" {
            String::new()
        } else {
            target()
        };
        append_transcript(transcript, &format!("> {text}"));
        append_transcript(transcript, "… waiting for reply");
        let snap = state().radio.snapshot();
        if snap.protocol_ver > 0 && snap.protocol_ver < 14 && contact.is_empty() {
            append_transcript(
                transcript,
                &format!(
                    "error: companion protocol v{} — local CLI needs v14+.",
                    snap.protocol_ver
                ),
            );
            return;
        }
        let expected_seq = snap.cli_seq;
        state().radio.run_cli(contact, text);
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
                    "error: no CLI reply (timeout). Needs companion protocol v14+, and the radio link must be free.".into()
                };
                append_transcript(transcript, &hint);
            }
        });
    };

    let heading_blurb = if page_tab() == CliPageTab::RxLog {
        "Live overheard RF frames from the companion (SNR / RSSI / route). Listening only while this tab is open."
    } else {
        "Run MeshCore CLI on this companion or a remote repeater, room / data server, or sensor. Command list follows the target role."
    };

    rsx! {
        div { class: "mx-auto flex h-full min-h-0 max-w-4xl flex-col gap-3",
            PageHeading { icon: IconName::Terminal, title: "Device CLI".to_string(),
                "{heading_blurb}"
            }
            div { class: "settings-subtabs",
                button {
                    r#type: "button",
                    class: if page_tab() == CliPageTab::Cli { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                    onclick: move |_| page_tab.set(CliPageTab::Cli),
                    "CLI"
                }
                button {
                    r#type: "button",
                    class: if page_tab() == CliPageTab::RxLog { "settings-subtab settings-subtab-active" } else { "settings-subtab" },
                    onclick: move |_| page_tab.set(CliPageTab::RxLog),
                    "RX log"
                }
            }
            if page_tab() == CliPageTab::Cli {
                if !linked {
                    p { class: "text-sm text-muted-foreground",
                        "Connect a companion radio to use the CLI."
                    }
                }
                div { class: "flex flex-wrap items-end gap-2",
                    div { class: "min-w-[14rem] flex-1",
                        label { class: "mb-1 block text-xs text-muted-foreground", "Target" }
                        SelectInput {
                            id: "cli-target".to_string(),
                            value: target(),
                            onchange: move |event: FormEvent| {
                                target.set(event.value());
                                preset.set(String::new());
                                draft.set(String::new());
                            },
                            option {
                                value: "local",
                                style: "background-color:#101418;color:#f4f4f5;",
                                "This companion"
                            }
                            for node in contacts.iter() {
                                option {
                                    key: "{node.pubkey}",
                                    value: "{node.pubkey}",
                                    style: "background-color:#101418;color:#f4f4f5;",
                                    "{node.name} · {contact_type_label(node.contact_type)}"
                                }
                            }
                        }
                    }
                    div { class: "flex items-center gap-2 pb-1 text-xs text-muted-foreground",
                        span { class: "rounded-md border border-border bg-card px-2 py-1",
                            "Mode: {mode.label()}"
                        }
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| transcript.set(Vec::new()),
                        "Clear"
                    }
                }
                div { class: "min-w-0",
                    label { class: "mb-1 block text-xs text-muted-foreground",
                        "Command ({mode.label()} presets — or type below)"
                    }
                    SelectInput {
                        id: "cli-preset".to_string(),
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
                pre { class: "cli-transcript",
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
                            // Keep preset in sync when the line still matches a catalog entry.
                            if catalog.iter().any(|(cmd, _)| *cmd == v) {
                                preset.set(v);
                            } else if !preset().is_empty() && preset() != v {
                                preset.set(String::new());
                            }
                        },
                    }
                    Button {
                        disabled: !linked || draft().trim().is_empty(),
                        onclick: move |_| {
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
                                send_command(text);
                            }
                        },
                        Icon { name: IconName::Check, size: 14 }
                        "Send"
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
                        "Send `{pending()}`? This may reboot or erase the target device."
                    }
                    div { class: "flex flex-wrap items-center gap-2",
                        Button {
                            kind: ButtonKind::Danger,
                            onclick: move |_| {
                                let text = pending();
                                confirm_open.set(false);
                                pending.set(String::new());
                                draft.set(String::new());
                                preset.set(String::new());
                                send_command(text);
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
            } else {
                // RX log tab
                if !linked {
                    p { class: "text-sm text-muted-foreground",
                        "Connect a companion radio to listen for RF frames."
                    }
                } else {
                    p { class: "text-sm text-muted-foreground",
                        "Listening for overheard packets… Leave this tab to stop (map upload may keep LogData if enabled)."
                    }
                }
                div { class: "flex flex-wrap items-center gap-2",
                    label { class: "flex cursor-pointer items-center gap-2 text-xs text-muted-foreground",
                        input {
                            r#type: "checkbox",
                            checked: rx_autoscroll(),
                            oninput: move |_| rx_autoscroll.set(!rx_autoscroll()),
                        }
                        "Auto-scroll"
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        onclick: move |_| {
                            state().radio.clear_rx_log();
                            rx_lines.set(Vec::new());
                            rx_seq.set(state().radio.rx_log_snapshot().0);
                        },
                        "Clear"
                    }
                    span { class: "text-xs text-muted-foreground",
                        "{rx_lines().len()} lines"
                    }
                }
                pre {
                    id: "hq-rx-log-transcript",
                    class: "cli-transcript",
                    if rx_lines().is_empty() {
                        span { class: "text-muted-foreground",
                            if linked {
                                "Waiting for packets…"
                            } else {
                                "No frames yet."
                            }
                        }
                    } else {
                        for line in rx_lines().iter() {
                            span {
                                key: "{line.seq}",
                                "{rx_log_clock(line.at)}  {line.text}\n"
                            }
                        }
                    }
                }
            }
        }
    }
}
