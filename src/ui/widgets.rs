use dioxus::html::FormValue;
use dioxus::prelude::*;

use super::icons::{Icon, IconName};
use crate::toast::{self, ToastKind};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonKind {
    #[default]
    Primary,
    Ghost,
    Danger,
}

#[component]
pub fn Button(
    #[props(default)] kind: ButtonKind,
    #[props(default = false)] disabled: bool,
    /// Use inside a `<form>` when `onsubmit` owns the action (Enter key works).
    #[props(default = false)]
    submit: bool,
    #[props(default)] onclick: EventHandler<MouseEvent>,
    children: Element,
) -> Element {
    let class = match kind {
        ButtonKind::Primary => "inline-flex h-9 shrink-0 items-center justify-center gap-2 whitespace-nowrap rounded-md bg-primary px-3 text-sm font-medium text-primary-foreground hover:opacity-90 disabled:opacity-40",
        ButtonKind::Ghost => "inline-flex h-9 shrink-0 items-center justify-center gap-2 whitespace-nowrap rounded-md border border-border bg-card px-3 text-sm text-foreground hover:bg-accent disabled:opacity-40",
        ButtonKind::Danger => "inline-flex h-9 shrink-0 items-center justify-center gap-2 whitespace-nowrap rounded-md bg-destructive px-3 text-sm font-medium text-white hover:opacity-90 disabled:opacity-40",
    };
    let button_type = if submit { "submit" } else { "button" };
    rsx! {
        button {
            // Default type=button so we do not double-fire with form onsubmit.
            r#type: "{button_type}",
            class: "{class} cursor-pointer",
            disabled,
            onclick: move |event| onclick.call(event),
            {children}
        }
    }
}

#[component]
pub fn Card(
    title: String,
    #[props(default)] icon: Option<IconName>,
    children: Element,
) -> Element {
    rsx! {
        section { class: "rounded-lg border border-border bg-card p-4",
            h2 { class: "mb-3 flex items-center gap-2 text-sm font-semibold tracking-wide text-foreground",
                if let Some(icon) = icon {
                    Icon { name: icon, size: 16 }
                }
                "{title}"
            }
            {children}
        }
    }
}

#[component]
pub fn PageHeading(icon: IconName, title: String, children: Element) -> Element {
    rsx! {
        div { class: "flex flex-col gap-1",
            h2 { class: "flex items-center gap-2 text-xl font-semibold",
                Icon { name: icon, size: 20 }
                "{title}"
            }
            div { class: "text-sm text-muted-foreground", {children} }
        }
    }
}

#[component]
pub fn Badge(tone: String, children: Element) -> Element {
    let class = match tone.as_str() {
        "pending" => "bg-amber-500/15 text-amber-200",
        "enrolled" | "hq" => "bg-emerald-500/15 text-emerald-200",
        "denied" => "bg-red-500/15 text-red-200",
        "ok" => "bg-sky-500/15 text-sky-200",
        "signal" => "bg-signal/15 text-signal",
        _ => "bg-muted text-muted-foreground",
    };
    rsx! {
        span { class: "inline-flex items-center rounded-md px-2 py-0.5 text-xs font-medium {class}",
            {children}
        }
    }
}

#[component]
pub fn TextInput(
    value: String,
    placeholder: String,
    #[props(default = "text".to_string())] kind: String,
    #[props(default)] maxlength: Option<u32>,
    #[props(default)] autofocus: bool,
    oninput: EventHandler<FormEvent>,
) -> Element {
    rsx! {
        input {
            class: "h-9 w-full rounded-md border border-border bg-card px-3 text-sm text-foreground outline-none",
            r#type: "{kind}",
            value,
            placeholder,
            maxlength: maxlength.map(|value| value.to_string()),
            autofocus,
            autocomplete: "off",
            oninput: move |event| oninput.call(event),
        }
    }
}

#[component]
pub fn BatteryPill(pct: Option<u8>, mv: Option<u16>, charging: bool) -> Element {
    let Some(pct) = pct else {
        return rsx! {
            span {
                class: "inline-flex items-center gap-1.5 text-muted-foreground",
                title: "Battery unknown",
                BatteryGlyph { pct: 0, charging: false, muted: true }
                span { class: "tabular-nums", "—" }
            }
        };
    };
    let fill = pct.min(100);
    let tone = if charging {
        "text-signal"
    } else if fill <= 15 {
        "text-red-200"
    } else if fill <= 35 {
        "text-amber-200"
    } else {
        "text-emerald-200"
    };
    let title = match mv {
        Some(mv) if charging => format!("Battery {pct}% · {mv} mV · charging"),
        Some(mv) => format!("Battery {pct}% · {mv} mV"),
        None if charging => format!("Battery {pct}% · charging"),
        None => format!("Battery {pct}%"),
    };
    rsx! {
        span {
            class: "inline-flex items-center gap-1.5 {tone}",
            title: "{title}",
            BatteryGlyph { pct: fill, charging, muted: false }
            span { class: "tabular-nums text-foreground", "{pct}%" }
        }
    }
}

#[component]
fn BatteryGlyph(pct: u8, charging: bool, muted: bool) -> Element {
    let fill = pct.min(100) as f32;
    // Inner cell is x=2..17 (15 wide), y=7..17 (10 tall) in a 24x24 viewBox-ish custom glyph.
    let inner_w = 13.0_f32;
    let filled_w = (inner_w * fill / 100.0).clamp(0.0, inner_w);
    let fill_color = if muted {
        "currentColor"
    } else if charging {
        "#3dd68c"
    } else if pct <= 15 {
        "#fca5a5"
    } else if pct <= 35 {
        "#fde68a"
    } else {
        "#6ee7b7"
    };
    rsx! {
        svg {
            class: "shrink-0",
            width: "20",
            height: "12",
            view_box: "0 0 24 14",
            fill: "none",
            // Shell
            rect {
                x: "1.25",
                y: "2.25",
                width: "18.5",
                height: "9.5",
                rx: "2",
                stroke: "currentColor",
                stroke_width: "1.5",
                opacity: if muted { "0.45" } else { "0.9" },
            }
            // Positive terminal
            rect {
                x: "20.25",
                y: "5",
                width: "2.25",
                height: "4",
                rx: "0.75",
                fill: "currentColor",
                opacity: if muted { "0.45" } else { "0.9" },
            }
            // Fill level
            if filled_w > 0.4 {
                rect {
                    x: "3",
                    y: "4",
                    width: "{filled_w:.2}",
                    height: "6",
                    rx: "1",
                    fill: "{fill_color}",
                    opacity: if muted { "0.35" } else { "0.95" },
                }
            }
            if charging {
                // Lightning bolt
                path {
                    d: "M11.2 3.2 8.4 7.6h2.1L9.1 11.2 13.6 6.4h-2.2z",
                    fill: if muted { "currentColor" } else { "#101418" },
                    stroke: if muted { "none" } else { "#3dd68c" },
                    stroke_width: "0.4",
                }
            }
        }
    }
}

#[component]
pub fn SelectInput(
    id: String,
    value: String,
    onchange: EventHandler<FormEvent>,
    children: Element,
) -> Element {
    rsx! {
        select {
            id: "{id}",
            class: "h-9 w-full max-w-full truncate rounded-md border border-border bg-card px-3 text-sm text-foreground outline-none",
            style: "color-scheme: dark; -webkit-appearance: none; appearance: none; background-color: #151a20; color: #f4f4f5; border-color: #ffffff24;",
            value,
            onchange: move |event| onchange.call(event),
            {children}
        }
    }
}

#[component]
pub fn Dialog(
    open: bool,
    title: String,
    on_close: EventHandler<()>,
    children: Element,
) -> Element {
    if !open {
        return rsx! {};
    }
    rsx! {
        div {
            class: "fixed inset-0 z-20 flex items-center justify-center bg-black/60 p-4",
            onclick: move |_| on_close.call(()),
            div {
                class: "w-full max-w-md rounded-lg border border-border bg-card p-4",
                onclick: move |event| event.stop_propagation(),
                h2 { class: "mb-3 text-base font-semibold", "{title}" }
                {children}
            }
        }
    }
}

/// Generic top-of-window slide-down panel (roster details, etc.).
///
/// Layout lives in `form.css` (`.slide-down-*`) because arbitrary Tailwind
/// max-height utilities are missing from the precompiled stylesheet.
#[component]
pub fn SlideDownPanel(
    open: bool,
    title: String,
    #[props(default)] subtitle: String,
    #[props(default)] icon: Option<IconName>,
    /// Wider sheet for dense tables (firmware Manage, etc.).
    #[props(default)]
    wide: bool,
    on_close: EventHandler<()>,
    children: Element,
) -> Element {
    let shell_class = if open {
        "slide-down-shell slide-down-shell-open"
    } else {
        "slide-down-shell"
    };
    let panel_class = if open {
        if wide {
            "slide-down-panel slide-down-panel-wide slide-down-panel-open"
        } else {
            "slide-down-panel slide-down-panel-open"
        }
    } else if wide {
        "slide-down-panel slide-down-panel-wide"
    } else {
        "slide-down-panel"
    };
    rsx! {
        div {
            class: "{shell_class}",
            // Soft dim so the roster stays visible but the sheet reads as a panel.
            if open {
                button {
                    class: "slide-down-backdrop",
                    r#type: "button",
                    title: "Close details",
                    "aria-label": "Close details",
                    onclick: move |_| on_close.call(()),
                }
            }
            div { class: "{panel_class}",
                div { class: "slide-down-header",
                    div { class: "min-w-0",
                        h2 { class: "flex items-center gap-2 text-base font-semibold",
                            if let Some(icon) = icon {
                                Icon { name: icon, size: 18 }
                            }
                            "{title}"
                        }
                        if !subtitle.is_empty() {
                            p { class: "mt-1 text-sm text-muted-foreground", "{subtitle}" }
                        }
                    }
                    button {
                        class: "inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground cursor-pointer",
                        r#type: "button",
                        title: "Close",
                        onclick: move |_| on_close.call(()),
                        Icon { name: IconName::Close, size: 16 }
                    }
                }
                div { class: "slide-down-body",
                    {children}
                }
            }
        }
    }
}

/// Top-of-window Bluetooth PIN prompt. The PIN field is uncontrolled so the
/// one-second status refresh cannot rewrite what the operator is typing.
#[component]
pub fn PinSlideDown(
    open: bool,
    target: String,
    error: String,
    form_key: u64,
    on_close: EventHandler<()>,
    on_submit: EventHandler<String>,
) -> Element {
    let panel_class = if open {
        "pointer-events-auto translate-y-0 opacity-100"
    } else {
        "pointer-events-none -translate-y-full opacity-0"
    };
    rsx! {
        div {
            class: "fixed inset-x-0 top-0 z-40 transition duration-300 ease-out {panel_class}",
            div { class: "border-b border-border bg-card/95 shadow-lg backdrop-blur-sm",
                div { class: "mx-auto flex max-w-3xl flex-col gap-3 px-5 py-4",
                    div { class: "flex items-start justify-between gap-3",
                        div { class: "min-w-0",
                            h2 { class: "flex items-center gap-2 text-base font-semibold",
                                Icon { name: IconName::Key, size: 18 }
                                "Bluetooth PIN"
                            }
                            p { class: "mt-1 text-sm text-muted-foreground",
                                if target.is_empty() {
                                    "Choose a radio first, then enter its six-digit Bluetooth PIN."
                                } else {
                                    "Enter the six-digit PIN for {target}. Radios with a screen show it on boot. Screenless radios often use 123456 unless you changed it."
                                }
                            }
                        }
                        button {
                            class: "inline-flex h-9 w-9 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground cursor-pointer",
                            r#type: "button",
                            title: "Close",
                            onclick: move |_| on_close.call(()),
                            Icon { name: IconName::Close, size: 16 }
                        }
                    }
                    if !target.is_empty() {
                        form {
                            key: "{form_key}",
                            class: "flex flex-col gap-3 sm:flex-row sm:items-end",
                            onsubmit: move |event: FormEvent| {
                                event.prevent_default();
                                let pin = event
                                    .get_first("pin")
                                    .and_then(|value| match value {
                                        FormValue::Text(text) => Some(text),
                                        FormValue::File(_) => None,
                                    })
                                    .unwrap_or_default();
                                on_submit.call(pin);
                            },
                            div { class: "min-w-0 flex-1",
                                label {
                                    class: "mb-1 block text-xs uppercase tracking-[0.16em] text-muted-foreground",
                                    r#for: "meshcore-ble-pin",
                                    "PIN"
                                }
                                input {
                                    id: "meshcore-ble-pin",
                                    name: "pin",
                                    class: "h-10 w-full rounded-md border border-border bg-background px-3 font-mono text-sm tracking-[0.35em] text-foreground outline-none focus:border-signal",
                                    r#type: "password",
                                    inputmode: "numeric",
                                    autocomplete: "off",
                                    autocapitalize: "off",
                                    spellcheck: false,
                                    maxlength: "6",
                                    placeholder: "••••••",
                                    autofocus: open,
                                }
                            }
                            div { class: "flex gap-2",
                                button {
                                    class: "inline-flex h-10 items-center justify-center gap-2 rounded-md bg-primary px-3 text-sm font-medium text-primary-foreground hover:opacity-90 cursor-pointer",
                                    r#type: "submit",
                                    Icon { name: IconName::Key, size: 14 }
                                    "Pair"
                                }
                                button {
                                    class: "inline-flex h-10 items-center justify-center gap-2 rounded-md border border-border bg-card px-3 text-sm text-foreground hover:bg-accent cursor-pointer",
                                    r#type: "button",
                                    onclick: move |_| on_close.call(()),
                                    Icon { name: IconName::Close, size: 14 }
                                    "Cancel"
                                }
                            }
                        }
                    }
                    if !error.is_empty() {
                        p { class: "text-sm text-red-200", "{error}" }
                    }
                }
            }
        }
    }
}

fn spark_color(tone: &str) -> &'static str {
    match tone {
        "signal" => "#3dd68c",
        "pending" => "#fcd34d",
        "ok" => "#7dd3fc",
        "tx" => "#f87171",
        "rx" => "#c084fc",
        _ => "#9aa3af",
    }
}

fn spark_paths(
    values: &[f32],
    width: f32,
    height: f32,
    zero_based: bool,
) -> Option<(String, String)> {
    if values.is_empty() {
        return None;
    }
    if zero_based && values.iter().all(|value| *value <= 0.0) {
        return None;
    }
    let (floor, span) = spark_scale(values, zero_based);
    let n = values.len();
    let step = if n <= 1 {
        0.0
    } else {
        width / (n as f32 - 1.0)
    };
    let mut line = String::new();
    for (index, value) in values.iter().enumerate() {
        let x = index as f32 * step;
        let y = height - ((value - floor) / span) * (height - 2.0) - 1.0;
        if index == 0 {
            line.push_str(&format!("M{x:.2} {y:.2}"));
        } else {
            line.push_str(&format!(" L{x:.2} {y:.2}"));
        }
    }
    let fill = format!("{line} L{width:.2} {height:.2} L0 {height:.2} Z");
    Some((line, fill))
}

#[component]
pub fn Sparkline(
    values: Vec<f32>,
    #[props(default = "muted".to_string())] tone: String,
    /// When false, scale to the series min/max (battery mV). Traffic stays zero-based.
    #[props(default = true)]
    zero_based: bool,
    /// Optional per-point labels for hover (e.g. hour ticks).
    #[props(default)]
    labels: Vec<String>,
    /// Shown after the numeric value in the tip (`12 msgs`).
    #[props(default)]
    value_suffix: String,
) -> Element {
    const W: f32 = 120.0;
    const H: f32 = 40.0;
    let mut hover = use_signal(|| None::<usize>);
    let color = spark_color(&tone);
    let paths = spark_paths(&values, W, H, zero_based);
    let n = values.len();
    let step = if n <= 1 { 0.0 } else { W / (n as f32 - 1.0) };
    let (floor, span) = spark_scale(&values, zero_based);
    let tip = hover().and_then(|idx| {
        values.get(idx).map(|v| {
            let label = labels
                .get(idx)
                .cloned()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("#{idx}"));
            let suffix = if value_suffix.is_empty() {
                String::new()
            } else {
                format!(" {value_suffix}")
            };
            (label, format!("{:.0}{suffix}", *v))
        })
    });
    let tip_style = match hover() {
        Some(idx) if n > 1 => {
            let pct = ((idx as f32 / (n as f32 - 1.0)) * 100.0).clamp(10.0, 90.0);
            format!("left: {pct:.1}%;")
        }
        _ => "left: 50%;".to_string(),
    };
    rsx! {
        div { class: "sparkline-chart",
            if let Some((label, vals)) = tip {
                div { class: "chart-tip", style: "{tip_style}",
                    p { class: "chart-tip-time", "{label}" }
                    p { class: "chart-tip-vals", "{vals}" }
                }
            }
            if let Some((line, fill)) = paths {
                svg {
                    class: "sparkline-chart-svg",
                    view_box: "0 0 {W} {H}",
                    preserve_aspect_ratio: "none",
                    onmouseleave: move |_| hover.set(None),
                    path {
                        d: "{fill}",
                        fill: "{color}",
                        fill_opacity: "0.22",
                        stroke: "none",
                        pointer_events: "none",
                    }
                    path {
                        d: "{line}",
                        fill: "none",
                        stroke: "{color}",
                        stroke_width: "1.75",
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                        pointer_events: "none",
                    }
                    if let Some(idx) = hover() {
                        {
                            let x = idx as f32 * step;
                            let y = H - ((values[idx] - floor) / span) * (H - 2.0) - 1.0;
                            rsx! {
                                line {
                                    x1: "{x}", y1: "0", x2: "{x}", y2: "{H}",
                                    stroke: "#ffffff33", stroke_width: "1",
                                    pointer_events: "none",
                                }
                                circle {
                                    cx: "{x}", cy: "{y}", r: "2.8",
                                    fill: "{color}", stroke: "#101418", stroke_width: "1.1",
                                    pointer_events: "none",
                                }
                            }
                        }
                    }
                    for (idx, _) in values.iter().enumerate() {
                        {
                            let x = if n <= 1 { 0.0 } else { idx as f32 * step };
                            let hit_w = if n <= 1 { W } else { (step * 0.9).max(4.0) };
                            let hit_x = (x - hit_w / 2.0).max(0.0);
                            rsx! {
                                rect {
                                    key: "{idx}",
                                    x: "{hit_x}", y: "0", width: "{hit_w}", height: "{H}",
                                    fill: "transparent",
                                    onmouseenter: move |_| hover.set(Some(idx)),
                                }
                            }
                        }
                    }
                }
            } else {
                div { class: "sparkline-chart-svg rounded bg-muted/40" }
            }
        }
    }
}

/// One battery telemetry sample with preformatted clock labels for the chart.
#[derive(Clone, PartialEq)]
pub struct BatterySample {
    pub at: i64,
    pub pct: f32,
    pub mv: f32,
    /// Short axis label (clock only).
    pub axis_label: String,
    /// Longer tooltip time (date + clock).
    pub tip_label: String,
}

/// Interactive companion battery voltage chart with time axis + hover tooltip.
#[component]
pub fn BatteryChart(points: Vec<BatterySample>) -> Element {
    const W: f32 = 240.0;
    const H: f32 = 56.0;
    let mut hover = use_signal(|| None::<usize>);
    if points.is_empty() {
        return rsx! {
            div { class: "mt-1.5 h-14 w-full rounded bg-muted/40" }
        };
    }
    let values: Vec<f32> = points.iter().map(|p| p.mv).collect();
    let color = spark_color("signal");
    let paths = spark_paths(&values, W, H, false);
    let n = points.len();
    let step = if n <= 1 { 0.0 } else { W / (n as f32 - 1.0) };
    let axis_idxs = battery_axis_indexes(n);
    let tip = hover().and_then(|idx| points.get(idx).cloned());
    let tip_style = match hover() {
        Some(idx) => {
            let pct = if n <= 1 {
                50.0
            } else {
                ((idx as f32 / (n as f32 - 1.0)) * 100.0).clamp(8.0, 92.0)
            };
            format!("left: {pct:.1}%;")
        }
        None => "left: 50%;".to_string(),
    };
    let (floor, span) = spark_scale(&values, false);
    rsx! {
        div { class: "battery-chart",
            if let Some(sample) = tip {
                div {
                    class: "battery-chart-tip",
                    style: "{tip_style}",
                    p { class: "battery-chart-tip-time", "{sample.tip_label}" }
                    p { class: "battery-chart-tip-vals",
                        "{sample.pct.round() as i32}% · {sample.mv.round() as i32} mV"
                    }
                }
            }
            if let Some((line, fill)) = paths {
                svg {
                    class: "battery-chart-svg",
                    view_box: "0 0 {W} {H}",
                    preserve_aspect_ratio: "none",
                    onmouseleave: move |_| hover.set(None),
                    path {
                        d: "{fill}",
                        fill: "{color}",
                        fill_opacity: "0.22",
                        stroke: "none",
                        pointer_events: "none",
                    }
                    path {
                        d: "{line}",
                        fill: "none",
                        stroke: "{color}",
                        stroke_width: "1.75",
                        stroke_linecap: "round",
                        stroke_linejoin: "round",
                        pointer_events: "none",
                    }
                    if let Some(idx) = hover() {
                        {
                            let x = idx as f32 * step;
                            let y = H - ((points[idx].mv - floor) / span) * (H - 2.0) - 1.0;
                            rsx! {
                                line {
                                    x1: "{x}",
                                    y1: "0",
                                    x2: "{x}",
                                    y2: "{H}",
                                    stroke: "#ffffff33",
                                    stroke_width: "1",
                                    pointer_events: "none",
                                }
                                circle {
                                    cx: "{x}",
                                    cy: "{y}",
                                    r: "3.2",
                                    fill: "{color}",
                                    stroke: "#101418",
                                    stroke_width: "1.25",
                                    pointer_events: "none",
                                }
                            }
                        }
                    }
                    for (idx, _) in points.iter().enumerate() {
                        {
                            let x = if n <= 1 { 0.0 } else { idx as f32 * step };
                            let hit_w = if n <= 1 { W } else { (step * 0.9).max(4.0) };
                            let hit_x = (x - hit_w / 2.0).max(0.0);
                            rsx! {
                                rect {
                                    key: "{idx}",
                                    x: "{hit_x}",
                                    y: "0",
                                    width: "{hit_w}",
                                    height: "{H}",
                                    fill: "transparent",
                                    onmouseenter: move |_| hover.set(Some(idx)),
                                }
                            }
                        }
                    }
                }
            } else {
                div { class: "battery-chart-svg rounded bg-muted/40" }
            }
            div { class: "battery-chart-axis",
                for idx in axis_idxs {
                    span { key: "{idx}", "{points[idx].axis_label}" }
                }
            }
        }
    }
}

fn spark_scale(values: &[f32], zero_based: bool) -> (f32, f32) {
    let data_max = values.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let data_min = values.iter().cloned().fold(f32::INFINITY, f32::min);
    if zero_based {
        (0.0, data_max.max(1.0))
    } else {
        let pad = ((data_max - data_min) * 0.15)
            .max(data_max.abs() * 0.005)
            .max(1.0);
        let floor = data_min - pad;
        (floor, (data_max + pad - floor).max(1.0))
    }
}

fn battery_axis_indexes(n: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![0];
    }
    if n == 2 {
        return vec![0, n - 1];
    }
    if n < 5 {
        return vec![0, n / 2, n - 1];
    }
    vec![0, n / 4, n / 2, (3 * n) / 4, n - 1]
}

#[derive(Clone, PartialEq)]
pub struct ChartSeries {
    pub label: String,
    pub tone: String,
    pub values: Vec<f32>,
}

#[component]
pub fn LineChart(
    series: Vec<ChartSeries>,
    #[props(default = 160.0)] height: f32,
    /// Sparse axis labels under the chart (not one per point).
    #[props(default)]
    labels: Vec<String>,
    /// One label per data point for hover tips (falls back to axis labels / index).
    #[props(default)]
    point_labels: Vec<String>,
) -> Element {
    const W: f32 = 640.0;
    let mut hover = use_signal(|| None::<usize>);
    let has_data = series
        .iter()
        .any(|row| row.values.iter().any(|value| *value > 0.0));
    if !has_data {
        return rsx! {
            p { class: "py-8 text-center text-sm text-muted-foreground", "No data yet" }
        };
    }
    let max = series
        .iter()
        .flat_map(|row| row.values.iter().copied())
        .fold(0.0_f32, f32::max)
        .max(1.0);
    let plot_bottom = height - 4.0;
    let n = series
        .iter()
        .map(|row| row.values.len())
        .max()
        .unwrap_or(0)
        .max(1);
    let step = if n <= 1 { 0.0 } else { W / (n as f32 - 1.0) };
    let tip = hover().map(|idx| {
        let when = point_labels
            .get(idx)
            .cloned()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("Point {}", idx + 1));
        let mut parts = Vec::new();
        for row in series.iter() {
            let v = row.values.get(idx).copied().unwrap_or(0.0);
            parts.push(format!("{} {:.0}", row.label, v));
        }
        (when, parts.join(" · "))
    });
    let tip_style = match hover() {
        Some(idx) if n > 1 => {
            let pct = ((idx as f32 / (n as f32 - 1.0)) * 100.0).clamp(8.0, 92.0);
            format!("left: {pct:.1}%;")
        }
        _ => "left: 50%;".to_string(),
    };
    rsx! {
        div { class: "line-chart flex flex-col gap-2",
            if let Some((when, vals)) = tip {
                div { class: "chart-tip", style: "{tip_style}",
                    p { class: "chart-tip-time", "{when}" }
                    p { class: "chart-tip-vals", "{vals}" }
                }
            }
            svg {
                class: "line-chart-svg",
                view_box: "0 0 {W} {height}",
                preserve_aspect_ratio: "none",
                onmouseleave: move |_| hover.set(None),
                line {
                    x1: "0",
                    y1: "{plot_bottom}",
                    x2: "{W}",
                    y2: "{plot_bottom}",
                    stroke: "#ffffff14",
                    stroke_width: "1",
                    pointer_events: "none",
                }
                for row in series.iter() {
                    {
                        let color = spark_color(&row.tone);
                        let mut line = String::new();
                        for (index, value) in row.values.iter().enumerate() {
                            let x = index as f32 * step;
                            let y = plot_bottom - (value / max) * (plot_bottom - 8.0) - 4.0;
                            if index == 0 {
                                line.push_str(&format!("M{x:.2} {y:.2}"));
                            } else {
                                line.push_str(&format!(" L{x:.2} {y:.2}"));
                            }
                        }
                        let fill = format!("{line} L{W:.2} {plot_bottom:.2} L0 {plot_bottom:.2} Z");
                        rsx! {
                            g {
                                key: "{row.label}",
                                pointer_events: "none",
                                path {
                                    d: "{fill}",
                                    fill: "{color}",
                                    fill_opacity: "0.14",
                                    stroke: "none",
                                }
                                path {
                                    d: "{line}",
                                    fill: "none",
                                    stroke: "{color}",
                                    stroke_width: "2.25",
                                    stroke_linecap: "round",
                                    stroke_linejoin: "round",
                                }
                            }
                        }
                    }
                }
                if let Some(idx) = hover() {
                    {
                        let x = idx as f32 * step;
                        rsx! {
                            line {
                                x1: "{x}", y1: "0", x2: "{x}", y2: "{height}",
                                stroke: "#ffffff33", stroke_width: "1",
                                pointer_events: "none",
                            }
                            for row in series.iter() {
                                {
                                    let color = spark_color(&row.tone);
                                    let value = row.values.get(idx).copied().unwrap_or(0.0);
                                    let y = plot_bottom - (value / max) * (plot_bottom - 8.0) - 4.0;
                                    rsx! {
                                        circle {
                                            key: "dot-{row.label}",
                                            cx: "{x}", cy: "{y}", r: "3.2",
                                            fill: "{color}", stroke: "#101418", stroke_width: "1.25",
                                            pointer_events: "none",
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                for idx in 0..n {
                    {
                        let x = if n <= 1 { 0.0 } else { idx as f32 * step };
                        let hit_w = if n <= 1 { W } else { (step * 0.9).max(6.0) };
                        let hit_x = (x - hit_w / 2.0).max(0.0);
                        rsx! {
                            rect {
                                key: "hit-{idx}",
                                x: "{hit_x}", y: "0", width: "{hit_w}", height: "{height}",
                                fill: "transparent",
                                onmouseenter: move |_| hover.set(Some(idx)),
                            }
                        }
                    }
                }
            }
            if !labels.is_empty() {
                div { class: "flex justify-between gap-2 text-[0.65rem] tabular-nums text-muted-foreground",
                    for label in labels.iter() {
                        span { "{label}" }
                    }
                }
            }
            div { class: "flex flex-wrap gap-3 text-xs text-muted-foreground",
                for row in series.iter() {
                    span { class: "inline-flex items-center gap-1.5",
                        span {
                            class: "inline-block h-2 w-2 rounded-full",
                            style: "background-color: {spark_color(&row.tone)};",
                        }
                        "{row.label}"
                    }
                }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct BarMixRow {
    pub label: String,
    pub count: i64,
    pub tone: String,
}

#[component]
pub fn BarMix(rows: Vec<BarMixRow>) -> Element {
    let mut hover = use_signal(|| None::<String>);
    if rows.is_empty() {
        return rsx! {
            p { class: "py-8 text-center text-sm text-muted-foreground", "No data yet" }
        };
    }
    let max = rows.iter().map(|row| row.count).max().unwrap_or(0).max(1) as f32;
    const CHART_H: f32 = 120.0;
    let tip = hover().and_then(|key| {
        rows.iter()
            .find(|row| row.label == key)
            .map(|row| (row.label.clone(), row.count))
    });
    rsx! {
        div { class: "bar-mix-chart",
            style: "display:flex;align-items:flex-end;justify-content:space-around;gap:0.75rem;min-height:10rem;padding:0.25rem 0.15rem 0;",
            if let Some((label, count)) = tip {
                div { class: "chart-tip", style: "left: 50%;",
                    p { class: "chart-tip-time", "{label}" }
                    p { class: "chart-tip-vals", "{count} messages" }
                }
            }
            for row in rows {
                {
                    let height_pct = ((row.count as f32 / max) * 100.0).clamp(0.0, 100.0);
                    let bar_h = if row.count <= 0 {
                        4.0
                    } else {
                        (CHART_H * height_pct / 100.0).max(6.0)
                    };
                    let color = spark_color(&row.tone);
                    let label_key = row.label.clone();
                    let label_enter = row.label.clone();
                    rsx! {
                        div {
                            key: "{label_key}",
                            class: "bar-mix-col",
                            onmouseenter: move |_| hover.set(Some(label_enter.clone())),
                            onmouseleave: move |_| hover.set(None),
                            span {
                                style: "font-size:0.75rem;font-weight:600;font-variant-numeric:tabular-nums;color:#f4f4f5;",
                                "{row.count}"
                            }
                            div {
                                style: "display:flex;width:100%;height:{CHART_H}px;align-items:flex-end;justify-content:center;",
                                div {
                                    style: "width:70%;max-width:2.75rem;height:{bar_h}px;border-radius:0.45rem 0.45rem 0.2rem 0.2rem;background:{color};box-shadow:0 0 10px {color}33;transition:height 0.3s ease, filter 0.15s ease;cursor:default;",
                                }
                            }
                            span {
                                style: "max-width:100%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:0.7rem;color:#9aa3af;text-align:center;",
                                "{row.label}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Common MeshCore / messaging emoji. Kept compact so packet budgets stay usable.
const EMOJI_PICKER_ROWS: &[(&str, &[&str])] = &[
    (
        "Reactions",
        &["👍", "👎", "❤️", "😂", "🎉", "👏", "🔥", "✅"],
    ),
    (
        "Faces",
        &["😀", "😊", "😎", "🤔", "😅", "😢", "😡", "🫡"],
    ),
    (
        "Mesh",
        &["📡", "📶", "📍", "🔋", "⚡", "🛰️", "🗺️", "💬"],
    ),
    (
        "More",
        &["🙏", "💪", "👀", "⚠️", "❌", "☀️", "🌧️", "🌙"],
    ),
];

/// Append `emoji` when the result still fits MeshCore's 160-byte text budget.
pub fn append_emoji(draft: &str, emoji: &str) -> String {
    if draft.len().saturating_add(emoji.len()) > 160 {
        draft.to_string()
    } else {
        format!("{draft}{emoji}")
    }
}

/// Smile button that opens a compact emoji popover for message composers.
#[component]
pub fn EmojiPickerButton(
    #[props(default = false)] disabled: bool,
    on_pick: EventHandler<String>,
) -> Element {
    let mut open = use_signal(|| false);
    rsx! {
        div { class: "emoji-picker-wrap",
            button {
                r#type: "button",
                class: "emoji-picker-trigger",
                title: "Insert emoji",
                disabled,
                onclick: move |event: MouseEvent| {
                    event.prevent_default();
                    event.stop_propagation();
                    if !disabled {
                        open.set(!open());
                    }
                },
                Icon { name: IconName::Emoji, size: 18 }
            }
            if open() {
                div {
                    class: "emoji-picker-panel",
                    role: "dialog",
                    "aria-label": "Emoji picker",
                    onclick: move |event: MouseEvent| event.stop_propagation(),
                    for (label, emojis) in EMOJI_PICKER_ROWS {
                        p { class: "emoji-picker-label", "{*label}" }
                        div { class: "emoji-picker-grid",
                            for emoji in *emojis {
                                button {
                                    r#type: "button",
                                    class: "emoji-picker-cell",
                                    title: "{*emoji}",
                                    onclick: {
                                        let glyph = (*emoji).to_string();
                                        move |event: MouseEvent| {
                                            event.prevent_default();
                                            event.stop_propagation();
                                            on_pick.call(glyph.clone());
                                            open.set(false);
                                        }
                                    },
                                    "{*emoji}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Fixed toast stack (bottom-right). Poll the toast store from the parent via `tick`.
#[component]
pub fn ToastHost(tick: u32) -> Element {
    let _ = tick;
    let toasts = toast::snapshot();
    if toasts.is_empty() {
        return rsx! {};
    }
    rsx! {
        div {
            class: "toast-host",
            role: "status",
            aria_live: "polite",
            for entry in toasts.into_iter() {
                {
                    let id = entry.id;
                    let icon = match entry.kind {
                        ToastKind::Success => IconName::Check,
                        ToastKind::Error => IconName::Ban,
                        ToastKind::Warn => IconName::Clock,
                        ToastKind::Info => IconName::Messages,
                    };
                    rsx! {
                        div {
                            key: "{id}",
                            class: "{entry.kind.class()}",
                            div { class: "toast-body",
                                span { class: "toast-icon",
                                    Icon { name: icon, size: 14 }
                                }
                                p { class: "toast-message", "{entry.message}" }
                            }
                            button {
                                class: "toast-dismiss",
                                r#type: "button",
                                title: "Dismiss",
                                onclick: move |_| toast::dismiss(id),
                                Icon { name: IconName::Close, size: 12 }
                            }
                        }
                    }
                }
            }
        }
    }
}
