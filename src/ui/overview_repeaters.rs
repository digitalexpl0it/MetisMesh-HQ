//! Overview → Repeaters fleet dashboard: summary tiles, compact cards, selected detail.

use dioxus::prelude::*;

use crate::db::{unix_now, ClaimedInfraRow, InfraStatusLatest, NodeRow};
use crate::timefmt;

use super::infra_page::open_infra_detail;
use super::icons::{Icon, IconName};
use super::state;
use super::widgets::{
    Badge, BarMix, BarMixRow, Button, ButtonKind, ChartSeries, Gauge, LineChart, SelectInput,
    Sparkline,
};
use super::Page;

const FRESH_PROBE_SECS: i64 = 30 * 60;
const ONLINE_SECS: i64 = 30 * 60;

const SETTING_CHART_WINDOW: &str = "overview_repeater_chart_window";
const SETTING_BATTERY_MODE: &str = "overview_repeater_kpi_battery_mode";
const SETTING_QUEUE_MODE: &str = "overview_repeater_kpi_queue_mode";

/// Normalize a chart window id; unknown → `24h`.
pub fn normalize_chart_window(id: &str) -> &'static str {
    match id {
        "15m" => "15m",
        "1h" => "1h",
        "6h" => "6h",
        "7d" => "7d",
        _ => "24h",
    }
}

/// Seconds for a chart window id (invalid → 24h).
pub fn chart_window_secs(id: &str) -> i64 {
    match normalize_chart_window(id) {
        "15m" => 900,
        "1h" => 3_600,
        "6h" => 21_600,
        "7d" => 604_800,
        _ => 86_400,
    }
}

/// Short label for chart titles (invalid → `24h`).
pub fn chart_window_label(id: &str) -> &'static str {
    normalize_chart_window(id)
}

/// Normalize KPI display mode; unknown → `chart`.
pub fn normalize_kpi_mode(id: &str) -> &'static str {
    match id {
        "gauge" => "gauge",
        "numeric" => "numeric",
        _ => "chart",
    }
}

fn load_setting_or(key: &str, fallback: &str) -> String {
    state()
        .db
        .setting(key)
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// Prefer first verified claim, else first claim.
pub fn default_repeater_selection(claimed: &[ClaimedInfraRow]) -> Option<String> {
    claimed
        .iter()
        .find(|r| r.is_verified())
        .or_else(|| claimed.first())
        .map(|r| r.pubkey.clone())
}

/// Keep current selection if still claimed; otherwise fall back.
pub fn resolve_repeater_selection(
    current: &str,
    claimed: &[ClaimedInfraRow],
) -> Option<String> {
    if !current.is_empty() && claimed.iter().any(|r| r.pubkey == current) {
        return Some(current.to_string());
    }
    default_repeater_selection(claimed)
}

fn claim_title(row: &ClaimedInfraRow, nodes: &[NodeRow]) -> String {
    if let Some(node) = nodes.iter().find(|n| n.pubkey == row.pubkey) {
        if node.name.trim().is_empty() {
            node.prefix.clone()
        } else {
            node.name.clone()
        }
    } else if !row.label.is_empty() {
        row.label.clone()
    } else {
        row.pubkey.chars().take(12).collect()
    }
}

fn battery_pct(mv: i64) -> i64 {
    ((mv.saturating_sub(3000) * 100) / 1200).min(100)
}

fn format_infra_uptime(secs: i64) -> String {
    let secs = secs.max(0);
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

fn format_infra_airtime(ms: i64) -> String {
    format_infra_uptime(ms.max(0) / 1000)
}

/// Prefer per-interval counter deltas so charts show rate; fall back to absolute.
fn counter_delta_per_hour(points: Vec<(i64, i64)>) -> Vec<f32> {
    if points.is_empty() {
        return Vec::new();
    }
    if points.len() == 1 {
        return vec![points[0].1.max(0) as f32];
    }
    let mut out = Vec::with_capacity(points.len().saturating_sub(1));
    for w in points.windows(2) {
        let dt = (w[1].0 - w[0].0).max(1) as f64;
        let d = (w[1].1 - w[0].1).max(0) as f64;
        out.push((d * 3600.0 / dt) as f32);
    }
    out
}

/// TX+RX airtime delta as percent of wall-clock between samples.
fn airtime_pct_series(points: &[(i64, i64, i64)]) -> Vec<f32> {
    if points.is_empty() {
        return Vec::new();
    }
    if points.len() == 1 {
        return vec![0.0];
    }
    let mut out = Vec::with_capacity(points.len().saturating_sub(1));
    for w in points.windows(2) {
        let dt_ms = ((w[1].0 - w[0].0).max(1) as f64) * 1000.0;
        let d_tx = (w[1].1 - w[0].1).max(0) as f64;
        let d_rx = (w[1].2 - w[0].2).max(0) as f64;
        let pct = ((d_tx + d_rx) / dt_ms * 100.0).clamp(0.0, 100.0);
        out.push(pct as f32);
    }
    out
}

fn format_time(seconds: Option<i64>) -> String {
    let Some(seconds) = seconds else {
        return "—".into();
    };
    timefmt::format_stamp(&state().db, seconds)
}

#[component]
fn WindowChoice(
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

#[component]
fn KpiModeChoice(
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

#[component]
pub fn OverviewRepeatersTab(on_navigate: EventHandler<Page>) -> Element {
    let mut tick = use_signal(|| 0u64);
    let mut refresh_busy = use_signal(|| false);
    let mut refresh_one_busy = use_signal(|| false);
    let mut refresh_note = use_signal(String::new);
    let mut selected = use_signal(String::new);
    let mut chart_window = use_signal(|| {
        normalize_chart_window(&load_setting_or(SETTING_CHART_WINDOW, "24h")).to_string()
    });
    let mut battery_mode = use_signal(|| {
        normalize_kpi_mode(&load_setting_or(SETTING_BATTERY_MODE, "chart")).to_string()
    });
    let mut queue_mode = use_signal(|| {
        normalize_kpi_mode(&load_setting_or(SETTING_QUEUE_MODE, "chart")).to_string()
    });
    let _ = tick();

    let claimed = state().db.list_claimed_infra().unwrap_or_default();
    let nodes = state().db.list_nodes().unwrap_or_default();
    let window_id = normalize_chart_window(&chart_window());
    let window_label = chart_window_label(window_id);
    let since = unix_now() - chart_window_secs(window_id);
    let now = unix_now();
    let latest_by_key: std::collections::HashMap<String, InfraStatusLatest> = state()
        .db
        .list_infra_status_latest()
        .unwrap_or_default()
        .into_iter()
        .map(|row| (row.pubkey.clone(), row))
        .collect();
    let linked = state().radio.snapshot().state == "connected";
    let probe_interval_mins = state().db.infra_probe_interval_mins();
    let probe_max = state().db.infra_probe_max_targets();
    let probe_policy = if probe_interval_mins > 0 {
        format!("Auto-probe every {probe_interval_mins} min (max {probe_max}/sweep).")
    } else {
        format!("Auto-probe on companion connect only (max {probe_max}/sweep).")
    };

    let select_key =
        resolve_repeater_selection(&selected(), &claimed).unwrap_or_default();

    if claimed.is_empty() {
        return rsx! {
            section { class: "rounded-lg border border-border bg-card/40 p-5",
                h3 { class: "text-sm font-semibold", "No claimed infrastructure yet" }
                p { class: "mt-2 text-sm text-muted-foreground",
                    "Claim a repeater, room, or sensor under Infrastructure, verify admin over LoRa, then refresh status to populate telemetry."
                }
                div { class: "mt-4",
                    Button {
                        onclick: move |_| on_navigate.call(Page::Infrastructure),
                        Icon { name: IconName::Signal, size: 14 }
                        "Open Infrastructure"
                    }
                }
            }
        };
    }

    let claimed_n = claimed.len();
    let verified_n = claimed.iter().filter(|r| r.is_verified()).count();
    let mut fresh_n = 0usize;
    let mut stale_n = 0usize;
    let mut lowest_batt: Option<(String, i64, i64)> = None;
    let mut worst_snr: Option<(String, f64)> = None;
    for row in &claimed {
        let title = claim_title(row, &nodes);
        match latest_by_key.get(&row.pubkey) {
            Some(s) if now - s.probed_at < FRESH_PROBE_SECS => {
                fresh_n += 1;
                let pct = battery_pct(s.battery_mv);
                if lowest_batt
                    .as_ref()
                    .map(|(_, mv, _)| s.battery_mv < *mv)
                    .unwrap_or(true)
                {
                    lowest_batt = Some((title.clone(), s.battery_mv, pct));
                }
                if worst_snr
                    .as_ref()
                    .map(|(_, snr)| s.snr < *snr)
                    .unwrap_or(true)
                {
                    worst_snr = Some((title.clone(), s.snr));
                }
            }
            Some(s) => {
                stale_n += 1;
                let pct = battery_pct(s.battery_mv);
                if lowest_batt
                    .as_ref()
                    .map(|(_, mv, _)| s.battery_mv < *mv)
                    .unwrap_or(true)
                {
                    lowest_batt = Some((title.clone(), s.battery_mv, pct));
                }
                if worst_snr
                    .as_ref()
                    .map(|(_, snr)| s.snr < *snr)
                    .unwrap_or(true)
                {
                    worst_snr = Some((title.clone(), s.snr));
                }
            }
            None => {
                stale_n += 1;
            }
        }
    }
    let lowest_label = lowest_batt
        .as_ref()
        .map(|(name, _, pct)| format!("{pct}% · {name}"))
        .unwrap_or_else(|| "—".into());
    let worst_snr_label = worst_snr
        .as_ref()
        .map(|(name, snr)| format!("{snr:.1} · {name}"))
        .unwrap_or_else(|| "—".into());

    let detail_row = claimed.iter().find(|r| r.pubkey == select_key).cloned();
    let detail_latest = latest_by_key.get(&select_key).cloned();
    let detail_samples = if select_key.is_empty() {
        Vec::new()
    } else {
        state()
            .db
            .infra_status_samples_since(&select_key, since)
            .unwrap_or_default()
    };

    rsx! {
        div { class: "flex flex-col gap-4",
            div { class: "flex flex-wrap items-center justify-between gap-2",
                div { class: "min-w-0",
                    p { class: "text-xs text-muted-foreground",
                        "Fleet status from LoRa binary probes (sparse — not a continuous scrape). Select a claimed node for charts."
                    }
                    p { class: "mt-0.5 text-[0.7rem] text-muted-foreground",
                        "{probe_policy} Change under Settings → Preferences → Claimed infrastructure."
                    }
                }
                div { class: "flex flex-wrap items-center gap-2",
                    if !refresh_note().is_empty() {
                        span { class: "text-xs text-muted-foreground", "{refresh_note}" }
                    }
                    Button {
                        kind: ButtonKind::Ghost,
                        disabled: refresh_busy() || !linked,
                        onclick: move |_| {
                            if refresh_busy() || !linked {
                                return;
                            }
                            refresh_busy.set(true);
                            refresh_note.set("Refreshing claimed status…".into());
                            let max = state().db.infra_probe_max_targets();
                            let targets: Vec<String> = state()
                                .db
                                .list_claimed_infra()
                                .unwrap_or_default()
                                .into_iter()
                                .filter(|r| r.is_verified())
                                .map(|r| r.pubkey)
                                .take(max)
                                .collect();
                            if targets.is_empty() {
                                refresh_note.set("No verified claims to probe.".into());
                                refresh_busy.set(false);
                                return;
                            }
                            spawn(async move {
                                let mut ok = 0usize;
                                let total = targets.len();
                                for (i, key) in targets.into_iter().enumerate() {
                                    if i > 0 {
                                        tokio::time::sleep(std::time::Duration::from_millis(1200))
                                            .await;
                                    }
                                    let prior_tx = state()
                                        .db
                                        .get_infra_status_latest(&key)
                                        .ok()
                                        .flatten()
                                        .and_then(|r| r.tx_power_dbm);
                                    let rx = state().radio.probe_infra_status(key.clone());
                                    match rx.await.unwrap_or_else(|_| Err("cancelled".into())) {
                                        Ok(info) => {
                                            crate::radio::persist_infra_status_probe(
                                                &state().db, &key, &info, prior_tx,
                                            );
                                            ok += 1;
                                        }
                                        Err(err) => {
                                            hq_log!(warn, "overview infra probe {key}: {err}");
                                        }
                                    }
                                }
                                refresh_note.set(format!("Refreshed {ok}/{total}."));
                                refresh_busy.set(false);
                                tick.with_mut(|n| *n += 1);
                            });
                        },
                        if refresh_busy() { "Refreshing…" } else { "Refresh all" }
                    }
                }
            }

            div { class: "overview-fleet-tiles",
                div { class: "overview-fleet-tile overview-fleet-tile--claimed",
                    p { class: "overview-fleet-tile__label", "Claimed" }
                    p { class: "overview-fleet-tile__value", "{claimed_n}" }
                }
                div { class: "overview-fleet-tile overview-fleet-tile--verified",
                    p { class: "overview-fleet-tile__label", "Verified" }
                    p { class: "overview-fleet-tile__value", "{verified_n}" }
                }
                div { class: "overview-fleet-tile overview-fleet-tile--fresh",
                    p { class: "overview-fleet-tile__label", "Fresh" }
                    p { class: "overview-fleet-tile__value", "{fresh_n}" }
                }
                div { class: "overview-fleet-tile overview-fleet-tile--stale",
                    p { class: "overview-fleet-tile__label", "Stale / never" }
                    p { class: "overview-fleet-tile__value", "{stale_n}" }
                }
                div { class: "overview-fleet-tile overview-fleet-tile--battery",
                    p { class: "overview-fleet-tile__label", "Lowest battery" }
                    p { class: "overview-fleet-tile__value overview-fleet-tile__value--sm", "{lowest_label}" }
                }
                div { class: "overview-fleet-tile overview-fleet-tile--snr",
                    p { class: "overview-fleet-tile__label", "Worst SNR" }
                    p { class: "overview-fleet-tile__value overview-fleet-tile__value--sm", "{worst_snr_label}" }
                }
            }

            div { class: "overview-repeater-grid",
                for row in claimed.iter() {
                    {
                        let title = claim_title(row, &nodes);
                        let node = nodes.iter().find(|n| n.pubkey == row.pubkey);
                        let heard = node.and_then(|n| n.last_heard);
                        let heard_label = format_time(heard);
                        let latest = latest_by_key.get(&row.pubkey);
                        let probe_age_ok = latest
                            .map(|s| now - s.probed_at <= ONLINE_SECS)
                            .unwrap_or(false);
                        let heard_ok = heard.map(|t| now - t <= ONLINE_SECS).unwrap_or(false);
                        let online = probe_age_ok || heard_ok;
                        let heard_stale = heard.map(|t| now - t > ONLINE_SECS).unwrap_or(true);
                        let key = row.pubkey.clone();
                        let key_select = key.clone();
                        let key_open = key.clone();
                        let kind = row.kind.clone();
                        let verified = row.is_verified();
                        let is_selected = select_key == key;
                        let card_class = if is_selected {
                            "overview-repeater-card overview-repeater-card--selected"
                        } else {
                            "overview-repeater-card"
                        };
                        rsx! {
                            section {
                                class: "{card_class}",
                                onclick: move |_| selected.set(key_select.clone()),
                                div { class: "flex flex-wrap items-start justify-between gap-2",
                                    div { class: "min-w-0 flex-1",
                                        div { class: "flex flex-wrap items-center gap-2",
                                            span {
                                                class: if online {
                                                    "overview-repeater-online overview-repeater-online--on"
                                                } else {
                                                    "overview-repeater-online"
                                                }
                                            }
                                            h3 { class: "truncate text-sm font-semibold", "{title}" }
                                            Badge {
                                                tone: match kind.as_str() {
                                                    "room" => "pending".to_string(),
                                                    "sensor" => "signal".to_string(),
                                                    _ => "muted".to_string(),
                                                },
                                                {
                                                    match kind.as_str() {
                                                        "room" => "Room",
                                                        "sensor" => "Sensor",
                                                        _ => "Repeater",
                                                    }
                                                }
                                            }
                                            if verified {
                                                Badge { tone: "enrolled".to_string(), "Verified" }
                                            } else {
                                                Badge { tone: "pending".to_string(), "Unverified" }
                                            }
                                            if heard_stale {
                                                Badge { tone: "pending".to_string(), "Stale" }
                                            }
                                        }
                                        p { class: "mt-1 font-mono text-[0.65rem] text-muted-foreground",
                                            "{key.chars().take(12).collect::<String>()}"
                                        }
                                        p { class: "mt-0.5 text-[0.7rem] text-muted-foreground",
                                            "Heard {heard_label}"
                                            if let Some(s) = latest {
                                                " · probed {format_time(Some(s.probed_at))}"
                                            }
                                        }
                                    }
                                    div { class: "flex shrink-0 gap-1",
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: move |evt: MouseEvent| {
                                                evt.stop_propagation();
                                                selected.set(key.clone());
                                            },
                                            "Select"
                                        }
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: move |evt: MouseEvent| {
                                                evt.stop_propagation();
                                                open_infra_detail(key_open.clone());
                                                on_navigate.call(Page::Infrastructure);
                                            },
                                            Icon { name: IconName::ChevronRight, size: 14 }
                                            "Open"
                                        }
                                    }
                                }
                                if let Some(s) = latest {
                                    {
                                        let pct = battery_pct(s.battery_mv);
                                        let tx = s
                                            .tx_power_dbm
                                            .map(|v| format!("{v} dBm"))
                                            .unwrap_or_else(|| "—".into());
                                        let queue = s
                                            .tx_queue_len
                                            .map(|v| v.to_string())
                                            .unwrap_or_else(|| "—".into());
                                        rsx! {
                                            div { class: "mt-3 grid grid-cols-2 gap-x-3 gap-y-2 text-xs",
                                                div {
                                                    span { class: "text-muted-foreground", "Battery " }
                                                    span { class: "tabular-nums text-foreground", "{pct}% · {s.battery_mv} mV" }
                                                    div { class: "overview-repeater-batt",
                                                        span { style: "width: {pct}%;" }
                                                    }
                                                }
                                                div {
                                                    span { class: "text-muted-foreground", "RSSI " }
                                                    span { class: "tabular-nums text-foreground", "{s.last_rssi}" }
                                                }
                                                div {
                                                    span { class: "text-muted-foreground", "Noise " }
                                                    span { class: "tabular-nums text-foreground", "{s.noise_floor} dBm" }
                                                }
                                                div {
                                                    span { class: "text-muted-foreground", "SNR " }
                                                    span { class: "tabular-nums text-foreground", "{s.snr:.1}" }
                                                }
                                                div {
                                                    span { class: "text-muted-foreground", "Uptime " }
                                                    span { class: "tabular-nums text-foreground", "{format_infra_uptime(s.uptime_s)}" }
                                                }
                                                div {
                                                    span { class: "text-muted-foreground", "TX " }
                                                    span { class: "tabular-nums text-foreground", "{tx}" }
                                                }
                                                div {
                                                    span { class: "text-muted-foreground", "Queue " }
                                                    span { class: "tabular-nums text-foreground", "{queue}" }
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    p { class: "mt-3 text-xs text-muted-foreground",
                                        "No status sample yet."
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if let Some(row) = detail_row {
                {
                    let title = claim_title(&row, &nodes);
                    let samples = detail_samples;
                    let latest = detail_latest;
                    let key = row.pubkey.clone();
                    let key_refresh = key.clone();
                    let key_open = key.clone();
                    let battery_vals: Vec<f32> =
                        samples.iter().map(|s| s.battery_mv as f32).collect();
                    let noise_vals: Vec<f32> =
                        samples.iter().map(|s| s.noise_floor as f32).collect();
                    let rssi_vals: Vec<f32> =
                        samples.iter().map(|s| s.last_rssi as f32).collect();
                    let snr_vals: Vec<f32> = samples.iter().map(|s| s.snr as f32).collect();
                    let queue_points: Vec<(String, f32)> = samples
                        .iter()
                        .filter_map(|s| {
                            s.tx_queue_len
                                .map(|v| (format_time(Some(s.at)), v as f32))
                        })
                        .collect();
                    let queue_vals: Vec<f32> =
                        queue_points.iter().map(|(_, v)| *v).collect();
                    let queue_labels: Vec<String> =
                        queue_points.iter().map(|(l, _)| l.clone()).collect();
                    let recv_rate = counter_delta_per_hour(
                        samples.iter().map(|s| (s.at, s.nb_recv)).collect(),
                    );
                    let flood_rate = counter_delta_per_hour(
                        samples.iter().map(|s| (s.at, s.flood_sent)).collect(),
                    );
                    let direct_rate = counter_delta_per_hour(
                        samples.iter().map(|s| (s.at, s.direct_sent)).collect(),
                    );
                    let air_pct = airtime_pct_series(
                        &samples
                            .iter()
                            .map(|s| (s.at, s.airtime_ms, s.rx_airtime_ms))
                            .collect::<Vec<_>>(),
                    );
                    let chart_labels: Vec<String> = {
                        let n = samples.len();
                        samples
                            .iter()
                            .enumerate()
                            .filter(|(i, _)| n <= 4 || *i == 0 || *i == n / 2 || *i + 1 == n)
                            .map(|(_, s)| format_time(Some(s.at)))
                            .collect()
                    };
                    let point_labels: Vec<String> =
                        samples.iter().map(|s| format_time(Some(s.at))).collect();
                    let packet_rows = latest
                        .as_ref()
                        .map(|s| {
                            vec![
                                BarMixRow {
                                    label: "Flood TX".to_string(),
                                    count: s.flood_sent,
                                    tone: "tx".to_string(),
                                },
                                BarMixRow {
                                    label: "Direct TX".to_string(),
                                    count: s.direct_sent,
                                    tone: "signal".to_string(),
                                },
                                BarMixRow {
                                    label: "Recv".to_string(),
                                    count: s.nb_recv,
                                    tone: "rx".to_string(),
                                },
                            ]
                        })
                        .unwrap_or_default();
                    let options: Vec<(String, String)> = claimed
                        .iter()
                        .map(|r| (r.pubkey.clone(), claim_title(r, &nodes)))
                        .collect();

                    rsx! {
                        section { class: "overview-repeater-detail",
                            div { class: "flex flex-wrap items-end justify-between gap-3",
                                div { class: "min-w-0 flex-1",
                                    label {
                                        class: "mb-1 block text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground",
                                        r#for: "overview-repeater-select",
                                        "Details for"
                                    }
                                    SelectInput {
                                        id: "overview-repeater-select".to_string(),
                                        value: select_key.clone(),
                                        onchange: move |evt: FormEvent| {
                                            selected.set(evt.value());
                                        },
                                        for (pk, label) in options {
                                            option { value: "{pk}", "{label}" }
                                        }
                                    }
                                    p { class: "mt-1 text-xs text-muted-foreground",
                                        "{title} · {key.chars().take(12).collect::<String>()}"
                                        if let Some(s) = latest.as_ref() {
                                            " · probed {format_time(Some(s.probed_at))} · {samples.len()} sample(s) in view"
                                        } else {
                                            " · no status sample yet"
                                        }
                                    }
                                }
                                div { class: "flex flex-col items-stretch gap-2 sm:items-end",
                                    div { class: "flex flex-wrap gap-2",
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            disabled: refresh_one_busy() || !linked || !row.is_verified(),
                                            onclick: move |_| {
                                                if refresh_one_busy() || !linked {
                                                    return;
                                                }
                                                refresh_one_busy.set(true);
                                                refresh_note.set("Refreshing node…".into());
                                                let key = key_refresh.clone();
                                                spawn(async move {
                                                    let prior_tx = state()
                                                        .db
                                                        .get_infra_status_latest(&key)
                                                        .ok()
                                                        .flatten()
                                                        .and_then(|r| r.tx_power_dbm);
                                                    let rx = state().radio.probe_infra_status(key.clone());
                                                    match rx.await.unwrap_or_else(|_| Err("cancelled".into())) {
                                                        Ok(info) => {
                                                            crate::radio::persist_infra_status_probe(
                                                                &state().db, &key, &info, prior_tx,
                                                            );
                                                            refresh_note.set("Node status updated.".into());
                                                        }
                                                        Err(err) => {
                                                            refresh_note.set(err);
                                                        }
                                                    }
                                                    refresh_one_busy.set(false);
                                                    tick.with_mut(|n| *n += 1);
                                                });
                                            },
                                            if refresh_one_busy() { "Refreshing…" } else { "Refresh node" }
                                        }
                                        Button {
                                            kind: ButtonKind::Ghost,
                                            onclick: move |_| {
                                                open_infra_detail(key_open.clone());
                                                on_navigate.call(Page::Infrastructure);
                                            },
                                            Icon { name: IconName::ChevronRight, size: 14 }
                                            "Open in Infrastructure"
                                        }
                                    }
                                    div { class: "flex flex-wrap items-center gap-1.5",
                                        span { class: "text-[0.65rem] uppercase tracking-[0.12em] text-muted-foreground mr-1",
                                            "Window"
                                        }
                                        WindowChoice {
                                            label: "15m",
                                            value: "15m",
                                            current: chart_window(),
                                            onpick: move |value: String| {
                                                let id = normalize_chart_window(&value).to_string();
                                                let _ = state().db.set_setting(SETTING_CHART_WINDOW, &id);
                                                chart_window.set(id);
                                                tick.with_mut(|n| *n += 1);
                                            },
                                        }
                                        WindowChoice {
                                            label: "1h",
                                            value: "1h",
                                            current: chart_window(),
                                            onpick: move |value: String| {
                                                let id = normalize_chart_window(&value).to_string();
                                                let _ = state().db.set_setting(SETTING_CHART_WINDOW, &id);
                                                chart_window.set(id);
                                                tick.with_mut(|n| *n += 1);
                                            },
                                        }
                                        WindowChoice {
                                            label: "6h",
                                            value: "6h",
                                            current: chart_window(),
                                            onpick: move |value: String| {
                                                let id = normalize_chart_window(&value).to_string();
                                                let _ = state().db.set_setting(SETTING_CHART_WINDOW, &id);
                                                chart_window.set(id);
                                                tick.with_mut(|n| *n += 1);
                                            },
                                        }
                                        WindowChoice {
                                            label: "24h",
                                            value: "24h",
                                            current: chart_window(),
                                            onpick: move |value: String| {
                                                let id = normalize_chart_window(&value).to_string();
                                                let _ = state().db.set_setting(SETTING_CHART_WINDOW, &id);
                                                chart_window.set(id);
                                                tick.with_mut(|n| *n += 1);
                                            },
                                        }
                                        WindowChoice {
                                            label: "7d",
                                            value: "7d",
                                            current: chart_window(),
                                            onpick: move |value: String| {
                                                let id = normalize_chart_window(&value).to_string();
                                                let _ = state().db.set_setting(SETTING_CHART_WINDOW, &id);
                                                chart_window.set(id);
                                                tick.with_mut(|n| *n += 1);
                                            },
                                        }
                                    }
                                }
                            }

                            if let Some(s) = latest.as_ref() {
                                {
                                    let pct = battery_pct(s.battery_mv);
                                    let tx_known = s.tx_power_dbm.is_some();
                                    let tx = s
                                        .tx_power_dbm
                                        .map(|v| format!("{v} dBm"))
                                        .unwrap_or_else(|| "—".into());
                                    let queue_n = s.tx_queue_len.unwrap_or(0);
                                    let queue = s
                                        .tx_queue_len
                                        .map(|v| v.to_string())
                                        .unwrap_or_else(|| "—".into());
                                    let queue_gauge_max = (queue_n as f32).max(16.0);
                                    let batt_mode = normalize_kpi_mode(&battery_mode());
                                    let q_mode = normalize_kpi_mode(&queue_mode());
                                    rsx! {
                                        div { class: "mt-4 grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-5",
                                            div { class: "overview-fleet-tile overview-fleet-tile--battery",
                                                div { class: "overview-kpi-tile-head",
                                                    p { class: "overview-fleet-tile__label", "Battery" }
                                                    div { class: "overview-kpi-mode-row",
                                                        KpiModeChoice {
                                                            label: "Chart",
                                                            value: "chart",
                                                            current: battery_mode(),
                                                            onpick: move |value: String| {
                                                                let id = normalize_kpi_mode(&value).to_string();
                                                                let _ = state().db.set_setting(SETTING_BATTERY_MODE, &id);
                                                                battery_mode.set(id);
                                                            },
                                                        }
                                                        KpiModeChoice {
                                                            label: "Gauge",
                                                            value: "gauge",
                                                            current: battery_mode(),
                                                            onpick: move |value: String| {
                                                                let id = normalize_kpi_mode(&value).to_string();
                                                                let _ = state().db.set_setting(SETTING_BATTERY_MODE, &id);
                                                                battery_mode.set(id);
                                                            },
                                                        }
                                                        KpiModeChoice {
                                                            label: "#",
                                                            value: "numeric",
                                                            current: battery_mode(),
                                                            onpick: move |value: String| {
                                                                let id = normalize_kpi_mode(&value).to_string();
                                                                let _ = state().db.set_setting(SETTING_BATTERY_MODE, &id);
                                                                battery_mode.set(id);
                                                            },
                                                        }
                                                    }
                                                }
                                                p { class: "overview-fleet-tile__value overview-fleet-tile__value--sm", "{pct}%" }
                                                p { class: "text-[0.7rem] text-muted-foreground tabular-nums", "{s.battery_mv} mV" }
                                                if batt_mode == "chart" && !battery_vals.is_empty() {
                                                    div { class: "overview-kpi-spark",
                                                        Sparkline {
                                                            values: battery_vals.clone(),
                                                            tone: "batt".to_string(),
                                                            zero_based: false,
                                                            labels: point_labels.clone(),
                                                            value_suffix: "mV".to_string(),
                                                        }
                                                    }
                                                } else if batt_mode == "gauge" {
                                                    div { class: "overview-kpi-gauge",
                                                        Gauge {
                                                            value: pct as f32,
                                                            min: 0.0,
                                                            max: 100.0,
                                                            tone: "batt".to_string(),
                                                        }
                                                    }
                                                }
                                            }
                                            div { class: "overview-fleet-tile overview-fleet-tile--noise",
                                                p { class: "overview-fleet-tile__label", "Noise" }
                                                p { class: "overview-fleet-tile__value overview-fleet-tile__value--sm", "{s.noise_floor} dBm" }
                                                if !noise_vals.is_empty() {
                                                    div { class: "overview-kpi-spark",
                                                        Sparkline {
                                                            values: noise_vals.clone(),
                                                            tone: "pending".to_string(),
                                                            zero_based: false,
                                                            labels: point_labels.clone(),
                                                            value_suffix: "dBm".to_string(),
                                                        }
                                                    }
                                                }
                                            }
                                            div { class: "overview-fleet-tile overview-fleet-tile--rssi",
                                                p { class: "overview-fleet-tile__label", "RSSI" }
                                                p { class: "overview-fleet-tile__value overview-fleet-tile__value--sm", "{s.last_rssi}" }
                                                if !rssi_vals.is_empty() {
                                                    div { class: "overview-kpi-spark",
                                                        Sparkline {
                                                            values: rssi_vals.clone(),
                                                            tone: "ok".to_string(),
                                                            zero_based: false,
                                                            labels: point_labels.clone(),
                                                        }
                                                    }
                                                }
                                            }
                                            div { class: "overview-fleet-tile overview-fleet-tile--snr",
                                                p { class: "overview-fleet-tile__label", "SNR" }
                                                p { class: "overview-fleet-tile__value overview-fleet-tile__value--sm", "{s.snr:.1}" }
                                                if !snr_vals.is_empty() {
                                                    div { class: "overview-kpi-spark",
                                                        Sparkline {
                                                            values: snr_vals.clone(),
                                                            tone: "rx".to_string(),
                                                            zero_based: false,
                                                            labels: point_labels.clone(),
                                                        }
                                                    }
                                                }
                                            }
                                            div { class: "overview-fleet-tile overview-fleet-tile--tx",
                                                p { class: "overview-fleet-tile__label", "TX power" }
                                                p { class: "overview-fleet-tile__value overview-fleet-tile__value--sm", "{tx}" }
                                                if !tx_known {
                                                    p { class: "text-[0.65rem] text-muted-foreground",
                                                        "Not in binary status — set via Infra radio CLI"
                                                    }
                                                }
                                            }
                                            div { class: "overview-fleet-tile overview-fleet-tile--uptime",
                                                p { class: "overview-fleet-tile__label", "Uptime" }
                                                p { class: "overview-fleet-tile__value overview-fleet-tile__value--sm", "{format_infra_uptime(s.uptime_s)}" }
                                            }
                                            div { class: "overview-fleet-tile overview-fleet-tile--air-tx",
                                                p { class: "overview-fleet-tile__label", "Airtime TX" }
                                                p { class: "overview-fleet-tile__value overview-fleet-tile__value--sm", "{format_infra_airtime(s.airtime_ms)}" }
                                            }
                                            div { class: "overview-fleet-tile overview-fleet-tile--air-rx",
                                                p { class: "overview-fleet-tile__label", "Airtime RX" }
                                                p { class: "overview-fleet-tile__value overview-fleet-tile__value--sm", "{format_infra_airtime(s.rx_airtime_ms)}" }
                                            }
                                            div { class: "overview-fleet-tile overview-fleet-tile--queue",
                                                div { class: "overview-kpi-tile-head",
                                                    p { class: "overview-fleet-tile__label", "Queue" }
                                                    div { class: "overview-kpi-mode-row",
                                                        KpiModeChoice {
                                                            label: "Chart",
                                                            value: "chart",
                                                            current: queue_mode(),
                                                            onpick: move |value: String| {
                                                                let id = normalize_kpi_mode(&value).to_string();
                                                                let _ = state().db.set_setting(SETTING_QUEUE_MODE, &id);
                                                                queue_mode.set(id);
                                                            },
                                                        }
                                                        KpiModeChoice {
                                                            label: "Gauge",
                                                            value: "gauge",
                                                            current: queue_mode(),
                                                            onpick: move |value: String| {
                                                                let id = normalize_kpi_mode(&value).to_string();
                                                                let _ = state().db.set_setting(SETTING_QUEUE_MODE, &id);
                                                                queue_mode.set(id);
                                                            },
                                                        }
                                                        KpiModeChoice {
                                                            label: "#",
                                                            value: "numeric",
                                                            current: queue_mode(),
                                                            onpick: move |value: String| {
                                                                let id = normalize_kpi_mode(&value).to_string();
                                                                let _ = state().db.set_setting(SETTING_QUEUE_MODE, &id);
                                                                queue_mode.set(id);
                                                            },
                                                        }
                                                    }
                                                }
                                                p { class: "overview-fleet-tile__value overview-fleet-tile__value--sm", "{queue}" }
                                                if q_mode == "chart" && !queue_vals.is_empty() {
                                                    div { class: "overview-kpi-spark",
                                                        Sparkline {
                                                            values: queue_vals.clone(),
                                                            tone: "warn".to_string(),
                                                            zero_based: true,
                                                            labels: queue_labels.clone(),
                                                        }
                                                    }
                                                } else if q_mode == "gauge" {
                                                    div { class: "overview-kpi-gauge",
                                                        Gauge {
                                                            value: queue_n as f32,
                                                            min: 0.0,
                                                            max: queue_gauge_max,
                                                            tone: "warn".to_string(),
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else {
                                p { class: "mt-4 text-sm text-muted-foreground",
                                    "No status sample yet — tap Refresh node (verified + connected), or open Infrastructure."
                                }
                            }

                            if !samples.is_empty() {
                                div { class: "mt-4 flex flex-col gap-4",
                                    if samples.len() < 2 {
                                        p { class: "text-xs text-muted-foreground",
                                            "Only one sample in the {window_label} window — refresh again later for multi-point trends."
                                        }
                                    }
                                    div { class: "overview-chart-panel overview-chart-panel--rf",
                                        p { class: "overview-chart-panel__title",
                                            "RF · noise / RSSI / SNR · {window_label}"
                                        }
                                        LineChart {
                                            series: vec![
                                                ChartSeries {
                                                    label: "Noise".to_string(),
                                                    tone: "pending".to_string(),
                                                    values: noise_vals.clone(),
                                                },
                                                ChartSeries {
                                                    label: "RSSI".to_string(),
                                                    tone: "ok".to_string(),
                                                    values: rssi_vals.clone(),
                                                },
                                                ChartSeries {
                                                    label: "SNR".to_string(),
                                                    tone: "rx".to_string(),
                                                    values: snr_vals.clone(),
                                                },
                                            ],
                                            height: 140.0,
                                            labels: chart_labels.clone(),
                                            point_labels: point_labels.clone(),
                                            zero_based: false,
                                        }
                                    }
                                    div { class: "overview-chart-panel overview-chart-panel--packets",
                                        p { class: "overview-chart-panel__title",
                                            "Packet rates · Δ / hour · {window_label}"
                                        }
                                        LineChart {
                                            series: vec![
                                                ChartSeries {
                                                    label: "Recv".to_string(),
                                                    tone: "rx".to_string(),
                                                    values: recv_rate,
                                                },
                                                ChartSeries {
                                                    label: "Flood TX".to_string(),
                                                    tone: "tx".to_string(),
                                                    values: flood_rate,
                                                },
                                                ChartSeries {
                                                    label: "Direct TX".to_string(),
                                                    tone: "signal".to_string(),
                                                    values: direct_rate,
                                                },
                                            ],
                                            height: 140.0,
                                            labels: chart_labels.clone(),
                                            point_labels: point_labels.clone(),
                                            zero_based: true,
                                        }
                                    }
                                    div { class: "overview-chart-panel overview-chart-panel--airtime",
                                        p { class: "overview-chart-panel__title",
                                            "Airtime usage · % of interval · {window_label}"
                                        }
                                        LineChart {
                                            series: vec![
                                                ChartSeries {
                                                    label: "Airtime %".to_string(),
                                                    tone: "air".to_string(),
                                                    values: air_pct,
                                                },
                                            ],
                                            height: 120.0,
                                            labels: chart_labels.clone(),
                                            point_labels: point_labels.clone(),
                                            zero_based: true,
                                        }
                                    }
                                    if !packet_rows.is_empty() {
                                        div { class: "overview-chart-panel overview-chart-panel--mix",
                                            p { class: "overview-chart-panel__title",
                                                "Packets · flood / direct / recv"
                                            }
                                            BarMix { rows: packet_rows }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pubkey: &str, verified: bool) -> ClaimedInfraRow {
        ClaimedInfraRow {
            pubkey: pubkey.into(),
            kind: "repeater".into(),
            label: String::new(),
            notes: String::new(),
            added_at: 1,
            verified_at: if verified { 10 } else { 0 },
        }
    }

    #[test]
    fn selection_prefers_verified_then_falls_back() {
        let claimed = vec![row("a", false), row("b", true), row("c", false)];
        assert_eq!(default_repeater_selection(&claimed).as_deref(), Some("b"));
        assert_eq!(
            resolve_repeater_selection("c", &claimed).as_deref(),
            Some("c")
        );
        assert_eq!(
            resolve_repeater_selection("gone", &claimed).as_deref(),
            Some("b")
        );
        assert_eq!(
            resolve_repeater_selection("", &claimed).as_deref(),
            Some("b")
        );
    }

    #[test]
    fn airtime_pct_between_samples() {
        let pts = vec![(100, 0, 0), (200, 5_000, 5_000)];
        let series = airtime_pct_series(&pts);
        assert_eq!(series.len(), 1);
        // 10_000 ms airtime over 100_000 ms wall → 10%
        assert!((series[0] - 10.0).abs() < 0.01);
    }

    #[test]
    fn chart_window_secs_and_label() {
        assert_eq!(chart_window_secs("15m"), 900);
        assert_eq!(chart_window_secs("1h"), 3_600);
        assert_eq!(chart_window_secs("6h"), 21_600);
        assert_eq!(chart_window_secs("24h"), 86_400);
        assert_eq!(chart_window_secs("7d"), 604_800);
        assert_eq!(chart_window_secs("nope"), 86_400);
        assert_eq!(chart_window_label("7d"), "7d");
        assert_eq!(chart_window_label("weird"), "24h");
        assert_eq!(normalize_chart_window("1h"), "1h");
        assert_eq!(normalize_chart_window(""), "24h");
    }

    #[test]
    fn kpi_mode_defaults_to_chart() {
        assert_eq!(normalize_kpi_mode("gauge"), "gauge");
        assert_eq!(normalize_kpi_mode("numeric"), "numeric");
        assert_eq!(normalize_kpi_mode("chart"), "chart");
        assert_eq!(normalize_kpi_mode(""), "chart");
        assert_eq!(normalize_kpi_mode("spark"), "chart");
    }
}
