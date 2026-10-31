//! In-app MeshCore CLI terminal (local companion + remote contacts).

use dioxus::prelude::*;

use super::icons::{Icon, IconName};
use super::state;
use super::widgets::{Button, ButtonKind, Dialog, PageHeading, SelectInput, TextInput};
use crate::radio::RadioStatus;

fn is_destructive_cli(command: &str) -> bool {
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

#[component]
pub fn CliPage(radio: RadioStatus) -> Element {
    let mut draft = use_signal(String::new);
    let mut target = use_signal(|| "local".to_string());
    let mut transcript = use_signal(|| {
        vec![
            "MeshCore CLI — local companion uses firmware v14+ opcode 66.".to_string(),
            "Remote targets send TXT_TYPE_CLI_COMMAND to a contact.".to_string(),
            "Docs: https://docs.meshcore.io/cli_commands/".to_string(),
        ]
    });
    let mut confirm_open = use_signal(|| false);
    let mut pending = use_signal(String::new);
    let mut last_seq = use_signal(|| 0u64);
    let mut poll = use_signal(|| 0u32);
    let mut poll_started = use_signal(|| false);

    if !poll_started() {
        poll_started.set(true);
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                poll.with_mut(|n| *n = n.wrapping_add(1));
            }
        });
    }

    let linked = radio.state == "connected";
    let contacts = state()
        .db
        .list_nodes()
        .unwrap_or_default()
        .into_iter()
        .filter(|n| {
            // 2 repeater, 3 room, 4 sensor — also allow enrolled companions for lab use.
            matches!(n.contact_type, 2 | 3 | 4) || n.enrollment.as_str() == "enrolled"
        })
        .collect::<Vec<_>>();

    // Poll radio status for CLI replies (avoid mutating signals during render).
    use_effect(move || {
        let _ = poll();
        let snap = state().radio.snapshot();
        if snap.cli_seq == 0 || snap.cli_seq == last_seq() || snap.cli_line.is_empty() {
            return;
        }
        last_seq.set(snap.cli_seq);
        append_transcript(transcript, &snap.cli_line);
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

    rsx! {
        div { class: "mx-auto flex h-full min-h-0 max-w-4xl flex-col gap-3",
            PageHeading { icon: IconName::Messages, title: "Device CLI".to_string(),
                "Run MeshCore CLI commands on this companion or a remote repeater / room / sensor."
            }
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
                        onchange: move |event: FormEvent| target.set(event.value()),
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
                                "{node.name} · type {node.contact_type}"
                            }
                        }
                    }
                }
                Button {
                    kind: ButtonKind::Ghost,
                    onclick: move |_| transcript.set(Vec::new()),
                    "Clear"
                }
            }
            div { class: "cli-chips",
                for cmd in ["clock", "neighbors", "advert", "get radio.freq", "get gps.en"].iter() {
                    {
                        let cmd = (*cmd).to_string();
                        rsx! {
                            button {
                                key: "{cmd}",
                                class: "map-chip",
                                disabled: !linked,
                                onclick: move |_| {
                                    draft.set(cmd.clone());
                                    send_command(cmd.clone());
                                },
                                "{cmd}"
                            }
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
                    placeholder: "clock",
                    oninput: move |event: FormEvent| draft.set(event.value()),
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
        }
    }
}
