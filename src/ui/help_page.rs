//! In-app Help — operator desk guide + shareable radio-user signup steps.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dioxus::prelude::*;

use super::icons::{Icon, IconName};
use super::state;
use super::widgets::{Button, ButtonKind, PageHeading};
use crate::secrets::{CHANNEL_NAME, NET_CHANNEL_NAME};

#[derive(Clone, Copy, PartialEq, Eq)]
struct HelpTopic {
    id: &'static str,
    label: &'static str,
}

const TOPICS: &[HelpTopic] = &[
    HelpTopic {
        id: "help-what",
        label: "What HQ is",
    },
    HelpTopic {
        id: "help-radio-users",
        label: "For radio users",
    },
    HelpTopic {
        id: "help-connect",
        label: "Connect companion",
    },
    HelpTopic {
        id: "help-enroll",
        label: "Enrollment desk",
    },
    HelpTopic {
        id: "help-channels",
        label: "Channels",
    },
    HelpTopic {
        id: "help-messages",
        label: "Messages & Inbox",
    },
    HelpTopic {
        id: "help-map",
        label: "Map, Tracker, Infra",
    },
    HelpTopic {
        id: "help-weather",
        label: "Weather & alerts",
    },
    HelpTopic {
        id: "help-local-api",
        label: "Local API",
    },
    HelpTopic {
        id: "help-cli-flasher",
        label: "CLI & Flasher",
    },
    HelpTopic {
        id: "help-backup",
        label: "Backup",
    },
    HelpTopic {
        id: "help-offline",
        label: "Offline runbook",
    },
];

fn sanitize_help_id(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

fn scroll_help_section(id: &str) {
    let safe = sanitize_help_id(id);
    if safe.is_empty() {
        return;
    }
    // Instant scroll on the shared main pane (smooth + click-lock froze the TOC).
    let _ = document::eval(&format!(
        r#"const el = document.getElementById('{safe}');
const scroller = document.querySelector('.hq-main');
if (el && scroller) {{
  const top =
    el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
  scroller.scrollTop = Math.max(0, top - 12);
}} else if (el) {{
  el.scrollIntoView({{ behavior: 'instant', block: 'start' }});
}}"#
    ));
}

fn help_topic_ids_js() -> String {
    TOPICS
        .iter()
        .map(|t| format!("\"{}\"", t.id))
        .collect::<Vec<_>>()
        .join(",")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// One-shot probe: which Help section is under the reading marker in `.hq-main`.
async fn probe_help_active_section() -> Option<&'static str> {
    let ids = help_topic_ids_js();
    let js = format!(
        r#"(function() {{
  const ids = [{ids}];
  const scroller = document.querySelector('.hq-main');
  if (!scroller || !document.querySelector('.help-page')) return '';
  const scrollTop = scroller.scrollTop;
  const maxScroll = Math.max(0, scroller.scrollHeight - scroller.clientHeight);
  if (scrollTop <= 8) return ids[0];
  const origin = scroller.getBoundingClientRect().top;
  const viewH = scroller.clientHeight;
  // Do not hard-pin the last id near max scroll — short trailing sections
  // (CLI, Backup) were skipped. Deepen the marker as we approach the end
  // so each section can still win in order.
  const remain = maxScroll - scrollTop;
  let markerY = Math.min(120, Math.max(72, viewH * 0.2));
  if (remain < viewH) {{
    markerY = Math.min(viewH * 0.72, Math.max(markerY, viewH - remain * 0.85));
  }}
  const marker = origin + markerY;
  let best = ids[0];
  for (let i = 0; i < ids.length; i++) {{
    const el = document.getElementById(ids[i]);
    if (!el) continue;
    if (el.getBoundingClientRect().top <= marker) best = ids[i];
  }}
  return best;
}})()"#
    );
    let result = document::eval(&format!("return {js};")).await.ok()?;
    let id = match result {
        serde_json::Value::String(s) if !s.is_empty() => s,
        _ => return None,
    };
    TOPICS.iter().find(|t| t.id == id).map(|t| t.id)
}

#[component]
pub fn HelpPage() -> Element {
    let mut active = use_signal(|| TOPICS[0].id);
    // After a TOC click, hold the highlight briefly while scroll settles.
    let mut hold_until_ms = use_signal(|| 0u128);
    let alive = use_hook(|| Arc::new(AtomicBool::new(true)));
    {
        let alive = alive.clone();
        use_drop(move || {
            alive.store(false, Ordering::SeqCst);
        });
    }
    {
        let alive = alive.clone();
        let mut active_spy = active;
        let hold_until_ms = hold_until_ms;
        let mut started = use_signal(|| false);
        if !started() {
            started.set(true);
            spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                while alive.load(Ordering::SeqCst) {
                    if now_ms() >= hold_until_ms() {
                        if let Some(id) = probe_help_active_section().await {
                            if active_spy() != id {
                                active_spy.set(id);
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            });
        }
    }
    let info_name = state()
        .db
        .info_channel_name()
        .unwrap_or_else(|_| CHANNEL_NAME.into());
    let net_name = state()
        .db
        .net_channel_name()
        .unwrap_or_else(|_| NET_CHANNEL_NAME.into());
    rsx! {
        div { class: "flex flex-col gap-4",
            PageHeading { icon: IconName::Help, title: "Help".to_string(),
                "How to run this headquarters and how radio users join your channels. Share the radio-user section with volunteers. Channel names below match Settings → Private channels."
            }
            div { class: "help-page",
                nav { class: "help-toc", aria_label: "Help topics",
                    p { class: "help-toc-title", "Topics" }
                    div { class: "help-toc-list",
                        for topic in TOPICS.iter().copied() {
                            {
                                let id = topic.id;
                                let is_active = active() == id;
                                rsx! {
                                    button {
                                        key: "{id}",
                                        r#type: "button",
                                        class: if is_active {
                                            "help-toc-item help-toc-item-active"
                                        } else {
                                            "help-toc-item"
                                        },
                                        onclick: move |_| {
                                            active.set(id);
                                            hold_until_ms.set(now_ms() + 450);
                                            scroll_help_section(id);
                                        },
                                        "{topic.label}"
                                    }
                                }
                            }
                        }
                    }
                }
                div { class: "help-articles",
                    section { id: "help-what", class: "help-section",
                        h3 { class: "help-section-title",
                            Icon { name: IconName::Dashboard, size: 16 }
                            "What HQ is"
                        }
                        p { "Metis Mesh HQ is a local app on this computer. It owns the MeshCore companion link, roster, private channel secrets, and bot — no cloud login." }
                        p { "Communities stay isolated until both headquarters later choose to link. Direct messages, the roster, and GPS tracks stay local." }
                        p { "Do not present HQ messages as FEMA, NWS, or law enforcement." }
                    }

                    section { id: "help-radio-users", class: "help-section help-section-share",
                        h3 { class: "help-section-title",
                            Icon { name: IconName::Radio, size: 16 }
                            "For radio users (share this)"
                        }
                        p { class: "help-callout",
                            "Give these steps to people with a MeshCore phone or companion. They talk to your HQ radio, not to this laptop’s UI."
                        }
                        ol { class: "help-steps",
                            li {
                                "In MeshCore, open a "
                                strong { "direct message" }
                                " to this headquarters’ companion (the radio HQ is connected to)."
                            }
                            li {
                                "Send "
                                code { "signup" }
                                " or "
                                code { "signup YourName" }
                                " (name optional, short)."
                            }
                            li { "Wait for an operator to Approve you in HQ Inbox. You should get a reply that you are pending, then two approval messages." }
                            li {
                                "Each approval DM names a private channel and a secret. On your radio, "
                                strong { "add those private channels" }
                                ": "
                                code { "{info_name}" }
                                " (bot / info) and "
                                code { "{net_name}" }
                                " (peer traffic)."
                            }
                            li {
                                "Send "
                                code { "status" }
                                " anytime to check enrollment. Bot commands like "
                                code { "#weather" }
                                " work only after you are enrolled, on "
                                code { "{info_name}" }
                                " — not as a signup DM."
                            }
                        }
                        p { class: "help-muted",
                            "HQ never shows the raw secret in its message list (it shows “[secret sent]”). Read the secret from the DM on your MeshCore device."
                        }
                    }

                    section { id: "help-connect", class: "help-section",
                        h3 { class: "help-section-title",
                            Icon { name: IconName::Bluetooth, size: 16 }
                            "Connect the companion"
                        }
                        p { "Settings → Connect. One pool supports Bluetooth, USB serial, and TCP (Wi-Fi / Ethernet companions, default port 5000)." }
                        ul { class: "help-bullets",
                            li { "Bluetooth: phone app must disconnect from that radio first; enter the six-digit companion PIN when asked." }
                            li { "USB: pick the port; sticky identity survives OS renumbering when possible." }
                            li { "TCP: host + port; no PIN." }
                        }
                        p { "Pick a "
                            strong { "primary" }
                            " companion. Enrollment secrets, bot replies, and operator DMs transmit from that radio only."
                        }
                    }

                    section { id: "help-enroll", class: "help-section",
                        h3 { class: "help-section-title",
                            Icon { name: IconName::Inbox, size: 16 }
                            "Enrollment desk"
                        }
                        p { "Heard nodes appear on Roster automatically. They stay “heard” until they DM "
                            code { "signup" }
                            "."
                        }
                        ul { class: "help-bullets",
                            li {
                                strong { "Inbox → Approve" }
                                " — enrolls them and queues two DMs with the "
                                code { "{info_name}" }
                                " and "
                                code { "{net_name}" }
                                " secrets."
                            }
                            li {
                                strong { "Deny" }
                                " — no secret sent. They may "
                                code { "signup" }
                                " again later (back to pending)."
                            }
                            li {
                                strong { "Roster → Revoke" }
                                " — sets them denied, "
                                strong { "rotates both" }
                                " "
                                code { "{info_name}" }
                                " and "
                                code { "{net_name}" }
                                " secrets, and re-sends the new secrets only to nodes still enrolled."
                            }
                        }
                        p { "Approve only people you trust. Secrets never belong in screenshots or chat logs." }
                    }

                    section { id: "help-channels", class: "help-section",
                        h3 { class: "help-section-title",
                            Icon { name: IconName::Key, size: 16 }
                            "Channels HQ controls"
                        }
                        ul { class: "help-bullets",
                            li {
                                code { "{info_name}" }
                                " — bot home: weather, quake, amber/alerts, "
                                code { "#help" }
                                ". Scheduled and operator-confirmed relays post here. Rename under Settings → Private channels."
                            }
                            li {
                                code { "{net_name}" }
                                " — peer / human private traffic. No HQ bot on this channel. Rename under Settings → Private channels."
                            }
                            li {
                                "Extra private channels — create under Settings / Roster, then "
                                strong { "Add channel" }
                                " on Roster to invite an enrolled node (sends that channel’s secret by DM)."
                            }
                        }
                        p { "A "
                            code { "#" }
                            " name on the mesh is a bot command, not a hash-channel id."
                        }
                    }

                    section { id: "help-messages", class: "help-section",
                        h3 { class: "help-section-title",
                            Icon { name: IconName::Messages, size: 16 }
                            "Messages & Inbox"
                        }
                        ul { class: "help-bullets",
                            li {
                                strong { "Messages" }
                                " — community channel traffic and direct threads."
                            }
                            li {
                                strong { "Inbox" }
                                " — operator queue: pending signups and held service alerts (when auto-relay is off)."
                            }
                        }
                        p { "Ordinary DMs live under Messages → Direct, not Inbox." }
                    }

                    section { id: "help-map", class: "help-section",
                        h3 { class: "help-section-title",
                            Icon { name: IconName::Map, size: 16 }
                            "Map, Tracker, Infrastructure"
                        }
                        ul { class: "help-bullets",
                            li {
                                strong { "Map" }
                                " — pins from self-reported GPS (ignore 0,0). HQ site pin is separate (Settings). Trace (tracked or claimed pins) fetches a path via the companion only — no channel/DM posts; uses airtime. Traffic overlay weights overheard edges in the time window (passive)."
                            }
                            li {
                                strong { "Tracker" }
                                " — operator whitelist of nodes expected to share location; trails stay local."
                            }
                            li {
                                strong { "Infrastructure" }
                                " — Claimed vs Claim tabs; verify admin over LoRa through the primary companion. Status refresh + Settings mirror the MeshCore app (position, clock, ACL, passwords, regions, TX). Claimed nodes are not pool companions."
                            }
                            li {
                                strong { "Overview → Repeaters" }
                                " — appears once infrastructure is claimed. Fleet tiles, card grid, and a Details for dropdown for KPIs/charts from LoRa status samples (sparse probes). Detail charts use a 15m–7d window; Battery and Queue tiles can switch Chart / Gauge / Numeric. Star KPI tiles to pin them first (saved preference). Charts hide hard-gate / Hampel outliers for scale; latest tile values stay raw. Refresh all / Refresh node on the page; auto-probe interval, sweep size, sample cap, and status purge under Settings → Preferences → Claimed infrastructure."
                            }
                        }
                    }

                    section { id: "help-weather", class: "help-section",
                        h3 { class: "help-section-title",
                            Icon { name: IconName::Weather, size: 16 }
                            "Weather & alerts"
                        }
                        p {
                            "Enrolled nodes use bot commands on "
                            code { "{info_name}" }
                            " ("
                            code { "#weather" }
                            ", "
                            code { "#quake" }
                            ", "
                            code { "#amber" }
                            ", "
                            code { "#alert" }
                            "). Settings → Services configures ZIP schedules, NWS / Amber polls, MQTT or Ecowitt stations, and auto-relay vs Inbox confirm. For Home Assistant / scripts, see "
                            strong { "Local API" }
                            " below."
                        }
                        p { "Label source and time. Mesh text stays plain — never treat it as an official agency product." }
                    }

                    section { id: "help-local-api", class: "help-section",
                        h3 { class: "help-section-title",
                            Icon { name: IconName::Settings, size: 16 }
                            "Local API"
                        }
                        p {
                            "Opt-in read-only JSON under Settings → Services → "
                            strong { "Local API" }
                            ". Default "
                            code { "127.0.0.1:8787" }
                            " with path prefix "
                            code { "/api/v1" }
                            ". Bearer token required on every request (stored as "
                            code { "data/local-api.token" }
                            ", mode 0600). Field names under "
                            code { "/api/v1" }
                            " are stable; breaking changes would use a new version prefix."
                        }
                        p {
                            strong { "Never exposed: " }
                            "channel secrets, keyring material, Compose/send, enroll/approve/deny, radio CLI, full message bodies, or GPS trails. Forbidden paths return "
                            code { "403" }
                            "."
                        }
                        p { class: "help-muted",
                            "Prefer loopback. Binding to the LAN means anyone with the token on that network can read status."
                        }
                        h4 { class: "help-subtitle", "curl" }
                        pre { class: "help-pre",
                            {"TOKEN=…   # paste from Settings → Local API → Copy token\ncurl -sS -H \"Authorization: Bearer $TOKEN\" \\\n  http://127.0.0.1:8787/api/v1/health\ncurl -sS -H \"Authorization: Bearer $TOKEN\" \\\n  http://127.0.0.1:8787/api/v1/status\ncurl -sS -H \"Authorization: Bearer $TOKEN\" \\\n  http://127.0.0.1:8787/api/v1/stats\ncurl -sS -H \"Authorization: Bearer $TOKEN\" \\\n  http://127.0.0.1:8787/api/v1/infra"}
                        }
                        h4 { class: "help-subtitle", "Home Assistant (REST)" }
                        p { class: "help-muted",
                            "Replace "
                            code { "YOUR_TOKEN" }
                            ". Infra sensors use the first claimed row ("
                            code { "$[0].latest…" }
                            "); adjust the JSON path if you have several claims."
                        }
                        pre { class: "help-pre",
                            {"# configuration.yaml (or a package)\nsensor:\n  - platform: rest\n    name: Metis companion state\n    resource: http://127.0.0.1:8787/api/v1/status\n    headers:\n      Authorization: Bearer YOUR_TOKEN\n    value_template: \"{{ value_json.companion.state }}\"\n    json_attributes_path: \"$.companion\"\n    json_attributes:\n      - connected_name\n      transport\n\n  - platform: rest\n    name: Metis enrolled count\n    resource: http://127.0.0.1:8787/api/v1/stats\n    headers:\n      Authorization: Bearer YOUR_TOKEN\n    value_template: \"{{ value_json.enrollment.enrolled }}\"\n\n  - platform: rest\n    name: Metis repeater battery mV\n    resource: http://127.0.0.1:8787/api/v1/infra\n    headers:\n      Authorization: Bearer YOUR_TOKEN\n    value_template: \"{{ value_json[0].latest.battery_mv }}\"\n    unit_of_measurement: mV\n\n  - platform: rest\n    name: Metis repeater noise\n    resource: http://127.0.0.1:8787/api/v1/infra\n    headers:\n      Authorization: Bearer YOUR_TOKEN\n    value_template: \"{{ value_json[0].latest.noise_floor }}\"\n    unit_of_measurement: dBm\n\n  - platform: rest\n    name: Metis repeater probed at\n    resource: http://127.0.0.1:8787/api/v1/infra\n    headers:\n      Authorization: Bearer YOUR_TOKEN\n    value_template: \"{{ value_json[0].latest.probed_at }}\""}
                        }
                        h4 { class: "help-subtitle", "Home Assistant (MQTT discovery)" }
                        p {
                            "Opt-in under Settings → Services → Local API → "
                            strong { "Home Assistant MQTT discovery" }
                            " (separate from Weather MQTT). HQ publishes retained discovery under your discovery prefix (default "
                            code { "homeassistant" }
                            ") and state under the state prefix (default "
                            code { "metis_mesh_hq" }
                            "). Same six entities as the REST examples: companion online/state, enrolled count, first claimed repeater battery/noise/probe time. Password file: "
                            code { "data/ha-mqtt.pass" }
                            " (0600)."
                        }
                        p { class: "help-muted",
                            "Enable the MQTT integration in Home Assistant and set its discovery prefix to match HQ. Example Mosquitto on the LAN:"
                        }
                        pre { class: "help-pre",
                            {"# mosquitto.conf (minimal LAN example)\nlistener 1883\nallow_anonymous true\n# Or set a user/password and enter them in HQ Settings.\n\n# HA → Settings → Devices & services → MQTT\n# Discovery prefix: homeassistant  (must match HQ)"}
                        }
                    }

                    section { id: "help-cli-flasher", class: "help-section",
                        h3 { class: "help-section-title",
                            Icon { name: IconName::Terminal, size: 16 }
                            "CLI & Flasher"
                        }
                        ul { class: "help-bullets",
                            li {
                                strong { "CLI" }
                                " — run MeshCore CLI on this companion or a remote repeater / room / sensor (role-aware command list)."
                            }
                            li {
                                strong { "Flasher" }
                                " — sync MeshCore community firmware while online; Manage selects boards/roles; wait for "
                                strong { "Ready for outage" }
                                "; Export / Import a "
                                code { ".zip" }
                                " pack for USB sticks. Flash is blocked if a checksum fails."
                            }
                        }
                    }

                    section { id: "help-backup", class: "help-section",
                        h3 { class: "help-section-title",
                            Icon { name: IconName::Key, size: 16 }
                            "Backup"
                        }
                        p { "Settings → Data → Backup: encrypted "
                            code { ".metisbak" }
                            " (passphrase) of the database and channel secrets. Prefer that over copying files by hand."
                        }
                        p { "Live files are "
                            code { "metis.db" }
                            " plus optional "
                            code { "metis.db-wal" }
                            " and "
                            code { "metis.db-shm" }
                            ". If you copy the data folder cold, include all three — or stop HQ first so SQLite can checkpoint."
                        }
                    }

                    section { id: "help-offline", class: "help-section",
                        h3 { class: "help-section-title",
                            Icon { name: IconName::Clock, size: 16 }
                            "Offline runbook — first hour without Internet"
                        }
                        p {
                            "One-page guide for church / community volunteers when the WAN is down. Keep this Help topic open on the HQ laptop (or print the screen)."
                        }

                        h4 { class: "help-subhead", "Before the outage (do this while online)" }
                        ol { class: "help-steps",
                            li { "Install Metis Mesh HQ and finish the setup wizard (Bluetooth / USB dialout / optional flashers)." }
                            li { "Connect the MeshCore companion radio (BLE, USB, or TCP)." }
                            li {
                                strong { "Settings → Flasher → Manage firmware" }
                                ": select the boards and roles you actually own → "
                                strong { "Save & sync" }
                                "."
                            }
                            li {
                                "Wait until Flasher says "
                                strong { "Ready for outage" }
                                ". Optionally "
                                strong { "Check integrity" }
                                ", then "
                                strong { "Export pack" }
                                " to a "
                                code { "MetisMeshHQ-firmware-*.zip" }
                                " on a USB stick."
                            }
                            li { "Leave the laptop on UPS or wall power; disable sleep while HQ is the desk radio." }
                        }

                        h4 { class: "help-subhead", "First hour offline" }
                        ol { class: "help-steps",
                            li {
                                strong { "Power" }
                                " — laptop on AC or UPS; companion radio powered. Do not let the OS sleep."
                            }
                            li {
                                strong { "Open HQ" }
                                " — start Metis Mesh HQ from the desktop / Start menu (no cloud login)."
                            }
                            li {
                                strong { "Radio" }
                                " — Settings → Connect: confirm the companion shows connected. If USB moved ports, Refresh ports and reconnect."
                            }
                            li {
                                strong { "Map" }
                                " — open Map; confirm local pins / last-heard nodes (self-reported GPS only)."
                            }
                            li {
                                strong { "Channel" }
                                " — open Messages; post a short check-in on your community channel so volunteers know HQ is live."
                            }
                            li {
                                strong { "Roster" }
                                " — approve pending enrollments only for people you trust; secrets are never shown in the log."
                            }
                            li {
                                strong { "Flash only from cache" }
                                " — Settings → Flasher. Flash is blocked if a file fails checksum. If the locker is empty, "
                                strong { "Import pack" }
                                " and pick the USB "
                                code { ".zip" }
                                "."
                            }
                        }

                        h4 { class: "help-subhead", "What the HQ computer needs on battery / UPS" }
                        table { class: "help-table",
                            thead {
                                tr {
                                    th { "Need" }
                                    th { "Guidance" }
                                }
                            }
                            tbody {
                                tr {
                                    td { "Runtime" }
                                    td { "Size the UPS for laptop + USB radio (often 2–4 hours minimum for a desk shift)." }
                                }
                                tr {
                                    td { "Sleep" }
                                    td { "Turn off suspend/hibernate while HQ is on duty." }
                                }
                                tr {
                                    td { "Display" }
                                    td { "Dim the screen to save power; keep the window open so status stays visible." }
                                }
                                tr {
                                    td { "Network" }
                                    td { "Internet is optional once firmware is synced. Mesh traffic is radio-local." }
                                }
                                tr {
                                    td { "Heat / dust" }
                                    td { "Keep vents clear; a closed closet kills laptops on UPS." }
                                }
                            }
                        }

                        h4 { class: "help-subhead", "Do / don’t" }
                        ul { class: "help-bullets",
                            li {
                                strong { "Do" }
                                " treat HQ as the local system of record (roster, map, bot, audit log)."
                            }
                            li {
                                strong { "Do" }
                                " flash only indexed, verified firmware from this machine’s locker."
                            }
                            li {
                                strong { "Don’t" }
                                " present HQ messages as FEMA, NWS, or law enforcement."
                            }
                            li {
                                strong { "Don’t" }
                                " share channel secrets in chat or screenshots."
                            }
                        }

                        h4 { class: "help-subhead", "Where things live" }
                        p {
                            "Settings → About shows version and the data folder path ("
                            code { "~/.MetisMeshHQ" }
                            " on Linux). The live database is "
                            code { "metis.db" }
                            " plus optional "
                            code { "metis.db-wal" }
                            " / "
                            code { "metis.db-shm" }
                            " — use Settings → Backup for a consistent copy, or include all three if you copy the folder cold."
                        }
                        p { class: "help-muted",
                            "This Help topic is the operator copy shipped in the app. Settings → About keeps a short reminder checklist."
                        }
                        div { class: "help-actions",
                            Button {
                                kind: ButtonKind::Ghost,
                                onclick: move |_| {
                                    active.set("help-radio-users");
                                    hold_until_ms.set(now_ms() + 450);
                                    scroll_help_section("help-radio-users");
                                },
                                "Jump to radio-user steps"
                            }
                        }
                    }
                }
            }
        }
    }
}
