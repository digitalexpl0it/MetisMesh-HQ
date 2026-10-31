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

    // Append radio CLI replies as they arrive.
    if radio.cli_seq != last_seq() && !radio.cli_line.is_empty() {
        last_seq.set(radio.cli_seq);
        let line = radio.cli_line.clone();
        transcript.with_mut(|lines| {
            for part in line.lines() {
                lines.push(part.to_string());
            }
            if lines.len() > 400 {
                let skip = lines.len() - 400;
                lines.drain(0..skip);
            }
        });
    }

    let mut send_command = move |text: String| {
        if !linked {
            transcript.with_mut(|lines| {
                lines.push("Not connected — pair a companion first.".into());
            });
            return;
        }
        let contact = if target() == "local" {
            String::new()
        } else {
            target()
        };
        state().radio.run_cli(contact, text);
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
                div { class: "flex flex-wrap gap-2",
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
