use dioxus::html::FormValue;
use dioxus::prelude::*;

use super::icons::{Icon, IconName};
use crate::db::unix_now;
use crate::toast::{self, ToastKind};

/// Last-heard older than this → GPS/locate icon turns red.
pub const HEARD_WEEK_SECS: i64 = 7 * 24 * 60 * 60;

/// True when the node was heard within the last week.
pub fn heard_within_week(last_heard: Option<i64>) -> bool {
    last_heard.is_some_and(|t| (unix_now() - t).max(0) <= HEARD_WEEK_SECS)
}

/// Green when fresh, red when silent longer than a week (or never heard).
pub fn heard_icon_class(last_heard: Option<i64>) -> &'static str {
    if heard_within_week(last_heard) {
        "text-signal"
    } else {
        "text-red-200"
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonKind {
    #[default]
    Primary,
    Ghost,
    Danger,
}

/// Inline busy indicator for scan / refresh actions.
#[component]
pub fn Throbber(#[props(default = 14)] size: u32) -> Element {
    rsx! {
        span {
            class: "hq-throbber",
            style: "width:{size}px;height:{size}px;",
            role: "status",
            aria_label: "Working",
        }
    }
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
    let is_password = kind.eq_ignore_ascii_case("password");
    rsx! {
        input {
            class: "h-9 w-full rounded-md border border-border bg-card px-3 text-sm text-foreground outline-none",
            r#type: "{kind}",
            value,
            placeholder,
            maxlength: maxlength.map(|value| value.to_string()),
            autofocus,
            autocomplete: "off",
            spellcheck: !is_password,
            oninput: move |event| oninput.call(event),
        }
    }
}

pub const COMPOSE_SPELLCHECK_KEY: &str = "compose_spellcheck";

/// Cached preference — avoid a SQLite read on every compose keystroke.
static COMPOSE_SPELLCHECK: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(true);
static COMPOSE_SPELLCHECK_LOADED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Whether Messages / Broadcast compose should use OS WebView spellcheck.
pub fn compose_spellcheck_enabled() -> bool {
    use std::sync::atomic::Ordering;
    if !COMPOSE_SPELLCHECK_LOADED.load(Ordering::Relaxed) {
        let on = crate::services::setting_bool(&super::state().db, COMPOSE_SPELLCHECK_KEY, true);
        COMPOSE_SPELLCHECK.store(on, Ordering::Relaxed);
        COMPOSE_SPELLCHECK_LOADED.store(true, Ordering::Relaxed);
    }
    COMPOSE_SPELLCHECK.load(Ordering::Relaxed)
}

/// Persist + cache the compose spellcheck preference (Settings toggle).
pub fn set_compose_spellcheck_enabled(on: bool) {
    use std::sync::atomic::Ordering;
    COMPOSE_SPELLCHECK.store(on, Ordering::Relaxed);
    COMPOSE_SPELLCHECK_LOADED.store(true, Ordering::Relaxed);
    crate::services::set_setting_bool(&super::state().db, COMPOSE_SPELLCHECK_KEY, on);
    #[cfg(all(feature = "desktop", target_os = "linux"))]
    crate::enable_linux_webview_spellcheck();
}

/// Channel / DM / Broadcast compose editor with optional OS spellcheck and a
/// native context menu (see `data-allow-context-menu` + main.rs boot script).
///
/// Always a `<textarea>` — browsers (and WebView2) do not show spell underlines
/// on single-line `<input>`, even with `spellcheck=true`.
///
/// Pass a `Signal` that **this** component (or a small dock sibling) reads — do
/// not read that signal from a parent that also paints the message list, or
/// every keystroke will re-render the thread (cursor jump + lag).
#[component]
pub fn ComposeField(
    mut text: Signal<String>,
    placeholder: String,
    #[props(default = false)] disabled: bool,
    #[props(default = false)] multiline: bool,
    /// Max UTF-8 bytes kept in the draft (default MeshCore 160). Channel compose
    /// passes a smaller budget so the HQ name prefix still fits on the wire.
    #[props(default = MESH_TEXT_MAX_BYTES)] max_bytes: usize,
    #[props(default)] class_name: Option<String>,
    #[props(default)] onkeydown: EventHandler<KeyboardEvent>,
    /// Fired after the draft Signal updates (e.g. clear a canned-reply selection).
    #[props(default)] on_change: EventHandler<String>,
) -> Element {
    let spellcheck = compose_spellcheck_enabled();
    let class = class_name.unwrap_or_else(|| {
        if multiline {
            "compose-field compose-field-multi min-w-0 flex-1".into()
        } else {
            "compose-field compose-field-single min-w-0 flex-1".into()
        }
    });
    let rows = if multiline { "4" } else { "1" };
    let spell_attr = if spellcheck { "true" } else { "false" };
    let shown = text();
    let max_bytes = max_bytes.max(1);
    rsx! {
        textarea {
            class: "{class}",
            value: "{shown}",
            placeholder,
            disabled,
            rows: "{rows}",
            lang: "en",
            autocomplete: "off",
            autocorrect: "off",
            autocapitalize: "off",
            spellcheck: "{spell_attr}",
            "data-allow-context-menu": "true",
            oninput: move |event: FormEvent| {
                let next = clamp_mesh_text_to(event.value(), max_bytes);
                if text.peek().as_str() != next {
                    text.set(next.clone());
                    on_change.call(next);
                }
            },
            onkeydown: move |event: KeyboardEvent| {
                if !multiline && event.key() == Key::Enter {
                    event.prevent_default();
                }
                onkeydown.call(event);
            },
        }
    }
}

/// Labeled on/off switch for Settings (replaces bare checkboxes).
#[component]
pub fn ToggleSwitch(
    checked: bool,
    label: String,
    #[props(default = false)] disabled: bool,
    #[props(default)] hint: Option<String>,
    onchange: EventHandler<bool>,
) -> Element {
    let row_class = if disabled {
        "toggle-row toggle-row-disabled"
    } else {
        "toggle-row"
    };
    let track_class = if checked {
        "toggle-track toggle-track-on"
    } else {
        "toggle-track"
    };
    rsx! {
        div { class: "{row_class}",
            button {
                r#type: "button",
                class: "{track_class}",
                role: "switch",
                aria_checked: checked.to_string(),
                aria_label: "{label}",
                disabled,
                onclick: move |_| {
                    if !disabled {
                        onchange.call(!checked);
                    }
                },
                span { class: "toggle-thumb" }
            }
            div { class: "toggle-copy min-w-0",
                span { class: "toggle-label", "{label}" }
                if let Some(hint) = hint {
                    p { class: "toggle-hint", "{hint}" }
                }
            }
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
    /// Pinned under the scrollable body (Save / Close actions).
    #[props(default)]
    footer: Option<Element>,
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
    let has_footer = footer.is_some();
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
                div {
                    class: if has_footer { "slide-down-body slide-down-body-with-footer" } else { "slide-down-body" },
                    {children}
                }
                if let Some(footer) = footer {
                    div { class: "slide-down-footer",
                        {footer}
                    }
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
        // Above welcome-overlay (z-index 80) so first-run Pair is usable.
        div {
            class: "pin-slide-shell fixed inset-x-0 top-0 transition duration-300 ease-out {panel_class}",
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
        "air" => "#2dd4bf",
        "batt" => "#86efac",
        "warn" => "#fb923c",
        _ => "#9aa3af",
    }
}

/// Fraction of a gauge arc in `[0, 1]` after clamping `value` into `[min, max]`.
pub fn gauge_fraction(value: f32, min: f32, max: f32) -> f32 {
    if !(max > min) {
        return 0.0;
    }
    ((value - min) / (max - min)).clamp(0.0, 1.0)
}

fn gauge_arc_path(cx: f32, cy: f32, r: f32, t0: f32, t1: f32) -> String {
    // Semicircle: t=0 at left (-π), t=1 at right (0). Angles in SVG y-down.
    let a0 = std::f32::consts::PI * (1.0 - t0);
    let a1 = std::f32::consts::PI * (1.0 - t1);
    let x0 = cx + r * a0.cos();
    let y0 = cy - r * a0.sin();
    let x1 = cx + r * a1.cos();
    let y1 = cy - r * a1.sin();
    let large = if (t1 - t0).abs() > 0.5 { 1 } else { 0 };
    format!("M{x0:.2} {y0:.2} A{r:.2} {r:.2} 0 {large} 1 {x1:.2} {y1:.2}")
}

/// Compact semicircle gauge for KPI tiles (battery %, queue depth, etc.).
#[component]
pub fn Gauge(
    value: f32,
    #[props(default = 0.0)] min: f32,
    #[props(default = 100.0)] max: f32,
    #[props(default = "muted".to_string())] tone: String,
) -> Element {
    const W: f32 = 120.0;
    const H: f32 = 64.0;
    const CX: f32 = 60.0;
    const CY: f32 = 58.0;
    const R: f32 = 46.0;
    let frac = gauge_fraction(value, min, max);
    let color = spark_color(&tone);
    let track = gauge_arc_path(CX, CY, R, 0.0, 1.0);
    let fill = if frac > 0.001 {
        Some(gauge_arc_path(CX, CY, R, 0.0, frac))
    } else {
        None
    };
    rsx! {
        div { class: "gauge-chart",
            svg {
                class: "gauge-chart-svg",
                view_box: "0 0 {W} {H}",
                path {
                    d: "{track}",
                    fill: "none",
                    stroke: "#ffffff18",
                    stroke_width: "8",
                    stroke_linecap: "round",
                }
                if let Some(d) = fill {
                    path {
                        d: "{d}",
                        fill: "none",
                        stroke: "{color}",
                        stroke_width: "8",
                        stroke_linecap: "round",
                    }
                }
            }
        }
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
    let color = spark_color("batt");
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
    /// When true (default), scale from 0 — message/traffic counts.
    /// When false, auto-scale to min/max so negative RF values (noise/RSSI/SNR) plot correctly.
    #[props(default = true)]
    zero_based: bool,
) -> Element {
    const W: f32 = 640.0;
    let mut hover = use_signal(|| None::<usize>);
    let flat: Vec<f32> = series
        .iter()
        .flat_map(|row| row.values.iter().copied())
        .collect();
    if flat.is_empty() {
        return rsx! {
            p { class: "py-8 text-center text-sm text-muted-foreground", "No data yet" }
        };
    }
    let (floor, span) = spark_scale(&flat, zero_based);
    let plot_bottom = height - 4.0;
    let plot_top = 8.0;
    let plot_h = (plot_bottom - plot_top).max(1.0);
    let y_of = |value: f32| -> f32 { plot_bottom - ((value - floor) / span) * plot_h };
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
            parts.push(format!("{} {v:.1}", row.label));
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
                            let x = if n <= 1 { W / 2.0 } else { index as f32 * step };
                            let y = y_of(*value);
                            if index == 0 {
                                line.push_str(&format!("M{x:.2} {y:.2}"));
                            } else {
                                line.push_str(&format!(" L{x:.2} {y:.2}"));
                            }
                        }
                        // Single-point series: draw a short horizontal segment so a dot is visible.
                        if n == 1 {
                            let x = W / 2.0;
                            let y = y_of(row.values.first().copied().unwrap_or(0.0));
                            line = format!("M{:.2} {y:.2} L{:.2} {y:.2}", x - 12.0, x + 12.0);
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
                        let x = if n <= 1 { W / 2.0 } else { idx as f32 * step };
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
                                    let y = y_of(value);
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
                        let x = if n <= 1 { W / 2.0 } else { idx as f32 * step };
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

/// Re-export shared catalog (keywords + sections) for the picker UI.
use super::emoji_catalog::{emoji_query_matches, EMOJI_PICKER_ROWS};

/// MeshCore plain-text budget (UTF-8 bytes, not Unicode scalars / graphemes).
pub const MESH_TEXT_MAX_BYTES: usize = 160;

/// Truncate to at most `max_bytes` UTF-8 bytes on a char boundary.
pub fn clamp_mesh_text_to(s: impl Into<String>, max_bytes: usize) -> String {
    let s = s.into();
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Truncate to at most [`MESH_TEXT_MAX_BYTES`] UTF-8 bytes on a char boundary.
pub fn clamp_mesh_text(s: impl Into<String>) -> String {
    clamp_mesh_text_to(s, MESH_TEXT_MAX_BYTES)
}

/// Append `emoji` when the result still fits `max_bytes` (UTF-8).
pub fn append_emoji_within(draft: &str, emoji: &str, max_bytes: usize) -> String {
    if draft.len().saturating_add(emoji.len()) > max_bytes {
        draft.to_string()
    } else {
        format!("{draft}{emoji}")
    }
}

/// Append `emoji` when the result still fits MeshCore's 160-byte text budget.
pub fn append_emoji(draft: &str, emoji: &str) -> String {
    append_emoji_within(draft, emoji, MESH_TEXT_MAX_BYTES)
}

/// Companion advert name budget (UTF-8 bytes). Keeps `CMD_SET_ADVERT_NAME` frames short.
pub const ADVERT_NAME_MAX_BYTES: usize = 32;

/// Truncate to at most [`ADVERT_NAME_MAX_BYTES`] UTF-8 bytes on a char boundary.
pub fn clamp_advert_name(s: impl Into<String>) -> String {
    let s = s.into();
    if s.len() <= ADVERT_NAME_MAX_BYTES {
        return s;
    }
    let mut end = ADVERT_NAME_MAX_BYTES;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Count emoji grapheme clusters in a display name (ZWJ sequences count as one).
pub fn name_emoji_count(name: &str) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    name.graphemes(true).filter(|g| grapheme_is_emoji(g)).count()
}

/// Validate a companion name the operator wants to set (welcome / future forms).
///
/// Empty names are rejected. At most one emoji may be *set* here; names already
/// on the radio with more emojis are left alone elsewhere.
pub fn validate_advert_name_draft(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Enter a companion name.".into());
    }
    if name_emoji_count(trimmed) > 1 {
        return Err("Use at most one emoji in the companion name.".into());
    }
    let clamped = clamp_advert_name(trimmed);
    if clamped.trim().is_empty() {
        return Err("Enter a companion name.".into());
    }
    Ok(clamped)
}

/// Insert a share snippet into the compose draft (space-separated, 160-byte clamp).
pub fn insert_mesh_snippet(draft: &str, snippet: &str) -> String {
    let snippet = snippet.trim();
    if snippet.is_empty() {
        return draft.to_string();
    }
    if draft.trim().is_empty() {
        return clamp_mesh_text(snippet);
    }
    clamp_mesh_text(format!("{} {}", draft.trim_end(), snippet))
}

/// HQ / node position line for mesh share (never 0,0).
pub fn format_position_share(label: &str, lat: f64, lon: f64) -> Option<String> {
    if !valid_share_coords(lat, lon) {
        return None;
    }
    let label = label.trim();
    let body = if label.is_empty() {
        format!("{lat:.5}, {lon:.5}")
    } else {
        format!("{label}: {lat:.5}, {lon:.5}")
    };
    Some(clamp_mesh_text(body))
}

/// True for finite lat/lon in range, excluding the 0,0 placeholder.
pub fn valid_share_coords(lat: f64, lon: f64) -> bool {
    lat.is_finite()
        && lon.is_finite()
        && (-90.0..=90.0).contains(&lat)
        && (-180.0..=180.0).contains(&lon)
        && !(lat.abs() < 1e-9 && lon.abs() < 1e-9)
}

/// Parse a `{label}: lat, lon` or bare `lat, lon` share (same shape as [`format_position_share`]).
pub fn parse_position_share(text: &str) -> Option<(String, f64, f64)> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Prefer "Label: lat, lon" so names with digits do not steal the coords.
    if let Some((label, rest)) = trimmed.rsplit_once(':') {
        if let Some((lat, lon)) = parse_lat_lon_pair(rest.trim()) {
            return Some((label.trim().to_string(), lat, lon));
        }
    }
    parse_lat_lon_pair(trimmed).map(|(lat, lon)| (String::new(), lat, lon))
}

fn parse_lat_lon_pair(text: &str) -> Option<(f64, f64)> {
    let (lat_s, lon_s) = text.split_once(',')?;
    let lat_s = lat_s.trim();
    let lon_s = lon_s.trim();
    let lat: f64 = lat_s.parse().ok()?;
    let lon: f64 = lon_s.parse().ok()?;
    // Shares use decimal degrees; require a few fraction digits to avoid "1, 2" noise.
    if frac_digits(lat_s) < 3 || frac_digits(lon_s) < 3 {
        return None;
    }
    if !valid_share_coords(lat, lon) {
        return None;
    }
    Some((lat, lon))
}

fn frac_digits(num: &str) -> usize {
    num.split_once('.')
        .map(|(_, frac)| frac.chars().take_while(|c| c.is_ascii_digit()).count())
        .unwrap_or(0)
}

/// Approx. match for "same map pin" (~55 m at the equator).
pub fn coords_near(a_lat: f64, a_lon: f64, b_lat: f64, b_lon: f64) -> bool {
    (a_lat - b_lat).abs() < 5e-4 && (a_lon - b_lon).abs() < 5e-4
}

/// Share text for a channel.
///
/// Public and hash channels become a `meshcore://channel/add?…` URI (joinable
/// well-known / derived secret). Private channels stay `Channel: {name}` —
/// never put a private secret in mesh text; use Invite for that.
pub fn format_channel_share(name: &str, role: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return String::new();
    }
    if name.eq_ignore_ascii_case("Public") {
        return clamp_mesh_text(crate::contact_qr::channel_add_uri(
            "Public",
            &crate::contact_qr::public_channel_secret(),
        ));
    }
    if role.eq_ignore_ascii_case("hash") {
        let secret = crate::mesh_regions::hash_channel_secret(name);
        return clamp_mesh_text(crate::contact_qr::channel_add_uri(name, &secret));
    }
    clamp_mesh_text(format!("Channel: {name}"))
}

/// Compose + menu actions (channel vs DM).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ComposePlusAction {
    MyContact,
    MyPosition,
    ShareChannel,
    ShareContact,
    ShareMapLocation,
}

/// Invisible full-viewport layer under a popover so a click outside dismisses it.
#[component]
pub fn PopoverDismissLayer(on_dismiss: EventHandler<()>) -> Element {
    rsx! {
        div {
            class: "popover-dismiss-layer",
            "aria-hidden": "true",
            onclick: move |event: MouseEvent| {
                event.prevent_default();
                event.stop_propagation();
                on_dismiss.call(());
            },
        }
    }
}

/// Plus button that lists share actions above the compose field.
#[component]
pub fn ComposePlusButton(
    #[props(default = false)] disabled: bool,
    on_action: EventHandler<ComposePlusAction>,
) -> Element {
    let mut open = use_signal(|| false);
    let items: [(ComposePlusAction, &'static str); 5] = [
        (ComposePlusAction::MyContact, "My contact info"),
        (ComposePlusAction::MyPosition, "My current position"),
        (ComposePlusAction::ShareChannel, "Share channel"),
        (ComposePlusAction::ShareContact, "Share contact"),
        (ComposePlusAction::ShareMapLocation, "Share location from map"),
    ];
    rsx! {
        div { class: "emoji-picker-wrap",
            button {
                r#type: "button",
                class: "emoji-picker-trigger",
                title: "Share actions",
                "aria-label": "Share actions",
                "aria-expanded": if open() { "true" } else { "false" },
                disabled,
                onclick: move |event: MouseEvent| {
                    event.prevent_default();
                    event.stop_propagation();
                    if !disabled {
                        open.set(!open());
                    }
                },
                Icon { name: IconName::Plus, size: 18 }
            }
            if open() {
                PopoverDismissLayer {
                    on_dismiss: move |_| open.set(false),
                }
                div {
                    class: "compose-plus-panel",
                    role: "menu",
                    "aria-label": "Share actions",
                    onclick: move |event: MouseEvent| event.stop_propagation(),
                    for (action, label) in items {
                        button {
                            r#type: "button",
                            class: "compose-plus-item",
                            role: "menuitem",
                            onclick: move |event: MouseEvent| {
                                event.prevent_default();
                                event.stop_propagation();
                                open.set(false);
                                on_action.call(action);
                            },
                            "{label}"
                        }
                    }
                }
            }
        }
    }
}

/// Live `n/max B` mesh text counter — UTF-8 draft bytes, green under the limit, red at max.
/// Channel compose passes a reduced `max_bytes` so the HQ name prefix still fits on the wire.
#[component]
pub fn MeshTextCount(
    text: String,
    #[props(default = MESH_TEXT_MAX_BYTES)] max_bytes: usize,
) -> Element {
    // Rust `str::len` is UTF-8 bytes (MeshCore's limit), not Unicode scalars.
    let n = text.len();
    let max_bytes = max_bytes.max(1);
    let at_max = n >= max_bytes;
    let title = if max_bytes < MESH_TEXT_MAX_BYTES {
        format!(
            "Draft UTF-8 bytes. Max {max_bytes} so the HQ name prefix still fits MeshCore’s {MESH_TEXT_MAX_BYTES}-byte wire limit (emoji/non-ASCII count as more than one)."
        )
    } else {
        format!(
            "MeshCore limit is {MESH_TEXT_MAX_BYTES} UTF-8 bytes (emoji and non-ASCII count as more than one)."
        )
    };
    rsx! {
        span {
            class: if at_max {
                "mesh-text-count mesh-text-count-max"
            } else {
                "mesh-text-count"
            },
            title: "{title}",
            "{n}/{max_bytes} B"
        }
    }
}

pub const AVATAR_PALETTE_LEN: usize = 8;

fn conversation_avatar_letter(label: &str) -> char {
    label
        .chars()
        .find(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('#')
}

/// Stable 0..AVATAR_PALETTE_LEN tone from a contact seed (pubkey or display name).
pub fn conversation_avatar_tone(seed: &str) -> usize {
    let mut hash: u32 = 2_166_132_261;
    for byte in seed.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    (hash as usize) % AVATAR_PALETTE_LEN
}

/// True when a Unicode scalar looks like part of an emoji / pictograph.
fn char_is_emoji_like(c: char) -> bool {
    matches!(
        c,
        '\u{00A9}'
            | '\u{00AE}'
            | '\u{203C}'
            | '\u{2049}'
            | '\u{2122}'
            | '\u{2139}'
            | '\u{2194}'..='\u{2199}'
            | '\u{21A9}'..='\u{21AA}'
            | '\u{231A}'..='\u{231B}'
            | '\u{2328}'
            | '\u{23CF}'
            | '\u{23E9}'..='\u{23F3}'
            | '\u{23F8}'..='\u{23FA}'
            | '\u{24C2}'
            | '\u{25AA}'..='\u{25AB}'
            | '\u{25B6}'
            | '\u{25C0}'
            | '\u{25FB}'..='\u{25FE}'
            | '\u{2600}'..='\u{27BF}'
            | '\u{2934}'..='\u{2935}'
            | '\u{2B05}'..='\u{2B07}'
            | '\u{2B1B}'..='\u{2B1C}'
            | '\u{2B50}'
            | '\u{2B55}'
            | '\u{3030}'
            | '\u{303D}'
            | '\u{3297}'
            | '\u{3299}'
            | '\u{1F000}'..='\u{1FAFF}'
    ) || matches!(c, '\u{1F1E6}'..='\u{1F1FF}') // regional indicators (flags)
}

fn grapheme_is_emoji(g: &str) -> bool {
    let mut has_pict = false;
    for c in g.chars() {
        if matches!(c, '\u{200D}' | '\u{FE0F}' | '\u{20E3}' | '\u{1F3FB}'..='\u{1F3FF}') {
            continue;
        }
        if char_is_emoji_like(c) {
            has_pict = true;
        } else if c.is_ascii_alphanumeric() {
            return false;
        }
    }
    has_pict
}

/// Up to two emoji graphemes from a MeshCore-style display name (custom avatar).
///
/// MeshCore apps treat emoji in the node name as the avatar instead of a
/// generated letter. We take the first two emoji grapheme clusters in order
/// (ZWJ sequences count as one).
pub fn name_avatar_emojis(name: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let mut out = String::new();
    let mut count = 0usize;
    for g in name.graphemes(true) {
        if grapheme_is_emoji(g) {
            out.push_str(g);
            count += 1;
            if count >= 2 {
                break;
            }
        }
    }
    out
}

/// Generated circle avatar: emoji(s) from `label` when present (MeshCore custom
/// avatar), otherwise first letter + color from stable `seed` hash.
#[component]
pub fn ConversationAvatar(
    label: String,
    seed: String,
    /// Smaller circle for rail rows.
    #[props(default = false)]
    compact: bool,
    /// Larger circle for thread headers inside the bubble.
    #[props(default = false)]
    large: bool,
) -> Element {
    use unicode_segmentation::UnicodeSegmentation;
    let emojis = name_avatar_emojis(&label);
    let letter = conversation_avatar_letter(&label);
    let tone = conversation_avatar_tone(&seed);
    let size = if large {
        "messages-avatar messages-avatar-lg"
    } else if compact {
        "messages-avatar messages-avatar-sm"
    } else {
        "messages-avatar"
    };
    if !emojis.is_empty() {
        let dual = emojis.graphemes(true).count() >= 2;
        let emoji_class = if dual {
            format!("{size} messages-avatar-emoji messages-avatar-emoji-dual")
        } else {
            format!("{size} messages-avatar-emoji")
        };
        return rsx! {
            span {
                class: "{emoji_class}",
                title: "{label}",
                aria_hidden: "true",
                "{emojis}"
            }
        };
    }
    rsx! {
        span {
            class: "{size} messages-avatar-{tone}",
            title: "{label}",
            aria_hidden: "true",
            "{letter}"
        }
    }
}

/// Smile button that opens a compact emoji popover for message composers.
///
/// Used for compose bars, section icons, and any other emoji entry point —
/// search keywords live in [`super::emoji_catalog`].
#[component]
pub fn EmojiPickerButton(
    #[props(default = false)] disabled: bool,
    /// Anchor the popover to the right when the trigger sits after the text box.
    #[props(default = false)]
    align_end: bool,
    /// Open the panel below the trigger (dialogs / toolbars). Default opens upward.
    #[props(default = false)]
    drop_down: bool,
    on_pick: EventHandler<String>,
) -> Element {
    let mut open = use_signal(|| false);
    let mut query = use_signal(String::new);
    let panel_class = match (align_end, drop_down) {
        (true, true) => "emoji-picker-panel emoji-picker-panel-end emoji-picker-panel-down",
        (true, false) => "emoji-picker-panel emoji-picker-panel-end",
        (false, true) => "emoji-picker-panel emoji-picker-panel-down",
        (false, false) => "emoji-picker-panel",
    };
    let trigger_title = if drop_down {
        "Choose emoji"
    } else {
        "Insert emoji"
    };
    let q = query();
    let searching = !q.trim().is_empty();
    let filtered: Vec<(&str, Vec<(&str, &str)>)> = EMOJI_PICKER_ROWS
        .iter()
        .filter_map(|(label, emojis)| {
            let hits: Vec<_> = emojis
                .iter()
                .copied()
                .filter(|(glyph, keywords)| emoji_query_matches(&q, label, glyph, keywords))
                .collect();
            if hits.is_empty() {
                None
            } else {
                Some((*label, hits))
            }
        })
        .collect();
    let match_count: usize = filtered.iter().map(|(_, hits)| hits.len()).sum();
    rsx! {
        div { class: "emoji-picker-wrap",
            button {
                r#type: "button",
                class: "emoji-picker-trigger",
                title: "{trigger_title}",
                disabled,
                onclick: move |event: MouseEvent| {
                    event.prevent_default();
                    event.stop_propagation();
                    if !disabled {
                        let next = !open();
                        open.set(next);
                        if next {
                            query.set(String::new());
                        }
                    }
                },
                Icon { name: IconName::Emoji, size: 18 }
            }
            if open() {
                PopoverDismissLayer {
                    on_dismiss: move |_| {
                        open.set(false);
                        query.set(String::new());
                    },
                }
                div {
                    class: "{panel_class}",
                    role: "dialog",
                    "aria-label": "Emoji picker",
                    onclick: move |event: MouseEvent| event.stop_propagation(),
                    div { class: "emoji-picker-search",
                        Icon { name: IconName::Scan, size: 13 }
                        input {
                            class: "emoji-picker-search-input",
                            r#type: "search",
                            value: "{q}",
                            placeholder: "Search coffee, signal, heart…",
                            "aria-label": "Search emoji",
                            autofocus: true,
                            oninput: move |e: FormEvent| query.set(e.value()),
                            onclick: move |event: MouseEvent| event.stop_propagation(),
                        }
                        if searching {
                            button {
                                r#type: "button",
                                class: "emoji-picker-search-clear",
                                title: "Clear search",
                                onclick: move |event: MouseEvent| {
                                    event.prevent_default();
                                    event.stop_propagation();
                                    query.set(String::new());
                                },
                                Icon { name: IconName::Close, size: 11 }
                            }
                        }
                    }
                    if searching {
                        p { class: "emoji-picker-label",
                            if match_count == 0 {
                                "No matches"
                            } else {
                                "Matches"
                            }
                        }
                        if match_count == 0 {
                            p { class: "emoji-picker-empty", "Try another word — coffee, signal, rain…" }
                        } else {
                            div { class: "emoji-picker-grid",
                                for (_label, hits) in filtered.iter() {
                                    for (emoji, keywords) in hits.iter() {
                                        {
                                            let glyph = (*emoji).to_string();
                                            let tip = (*keywords).to_string();
                                            rsx! {
                                                button {
                                                    r#type: "button",
                                                    class: "emoji-picker-cell",
                                                    title: "{tip}",
                                                    onclick: move |event: MouseEvent| {
                                                        event.prevent_default();
                                                        event.stop_propagation();
                                                        on_pick.call(glyph.clone());
                                                        open.set(false);
                                                        query.set(String::new());
                                                    },
                                                    "{glyph}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    } else {
                        for (label, hits) in filtered.iter() {
                            p { class: "emoji-picker-label", "{*label}" }
                            div { class: "emoji-picker-grid",
                                for (emoji, keywords) in hits.iter() {
                                    {
                                        let glyph = (*emoji).to_string();
                                        let tip = (*keywords).to_string();
                                        rsx! {
                                            button {
                                                r#type: "button",
                                                class: "emoji-picker-cell",
                                                title: "{tip}",
                                                onclick: move |event: MouseEvent| {
                                                    event.prevent_default();
                                                    event.stop_propagation();
                                                    on_pick.call(glyph.clone());
                                                    open.set(false);
                                                    query.set(String::new());
                                                },
                                                "{glyph}"
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

#[cfg(test)]
mod avatar_emoji_tests {
    use super::name_avatar_emojis;

    #[test]
    fn leading_emoji_becomes_avatar() {
        assert_eq!(name_avatar_emojis("🔥Rose01"), "🔥");
        assert_eq!(name_avatar_emojis("🦊🐱 Fox Den"), "🦊🐱");
    }

    #[test]
    fn caps_at_two_emojis() {
        assert_eq!(name_avatar_emojis("😀😃😄 name"), "😀😃");
    }

    #[test]
    fn mid_name_emoji_still_used() {
        assert_eq!(name_avatar_emojis("HQ 🛰️ North"), "🛰️");
    }

    #[test]
    fn plain_name_has_no_emoji_avatar() {
        assert!(name_avatar_emojis("Rose01-rptr").is_empty());
        assert!(name_avatar_emojis("").is_empty());
    }
}

#[cfg(test)]
mod gauge_tests {
    use super::gauge_fraction;

    #[test]
    fn gauge_fraction_clamps_to_unit_interval() {
        assert!((gauge_fraction(50.0, 0.0, 100.0) - 0.5).abs() < 0.001);
        assert!((gauge_fraction(-10.0, 0.0, 100.0) - 0.0).abs() < 0.001);
        assert!((gauge_fraction(200.0, 0.0, 100.0) - 1.0).abs() < 0.001);
        assert!((gauge_fraction(5.0, 10.0, 10.0) - 0.0).abs() < 0.001);
    }
}

#[cfg(test)]
mod advert_name_tests {
    use super::{
        clamp_advert_name, name_emoji_count, validate_advert_name_draft, ADVERT_NAME_MAX_BYTES,
    };

    #[test]
    fn emoji_count_treats_zwj_as_one() {
        assert_eq!(name_emoji_count("HQ"), 0);
        assert_eq!(name_emoji_count("HQ🔥"), 1);
        assert_eq!(name_emoji_count("🔥HQ⭐"), 2);
    }

    #[test]
    fn validate_allows_one_emoji() {
        let ok = validate_advert_name_draft("  Metis🔥  ").unwrap();
        assert_eq!(ok, "Metis🔥");
        assert!(validate_advert_name_draft("").is_err());
        assert!(validate_advert_name_draft("🔥⭐").is_err());
    }

    #[test]
    fn clamp_advert_respects_budget() {
        let long = "a".repeat(ADVERT_NAME_MAX_BYTES + 8);
        let out = clamp_advert_name(long);
        assert!(out.len() <= ADVERT_NAME_MAX_BYTES);
    }
}

#[cfg(test)]
mod compose_share_tests {
    use super::{
        format_channel_share, format_position_share, insert_mesh_snippet, MESH_TEXT_MAX_BYTES,
    };

    #[test]
    fn position_share_skips_origin() {
        assert!(format_position_share("HQ", 0.0, 0.0).is_none());
        let text = format_position_share("HQ", 34.1, -117.2).unwrap();
        assert!(text.contains("34.10000"));
        assert!(text.starts_with("HQ:"));
    }

    #[test]
    fn channel_share_private_is_name_only() {
        let text = format_channel_share("highdesert", "general");
        assert_eq!(text, "Channel: highdesert");
        assert!(!text.contains("meshcore://"));
    }

    #[test]
    fn channel_share_public_is_add_uri() {
        let text = format_channel_share("Public", "general");
        assert!(text.starts_with("meshcore://channel/add?"));
        assert!(text.contains("name=Public"));
        assert!(text.contains("secret=8b3387e9c5cdea6ac9e5edbaa115cd72"));
    }

    #[test]
    fn channel_share_hash_is_add_uri() {
        let text = format_channel_share("highdesert", "hash");
        assert!(text.starts_with("meshcore://channel/add?"));
        assert!(text.contains("name=highdesert"));
        assert!(text.contains("secret="));
    }

    #[test]
    fn insert_snippet_respects_budget() {
        let long = "a".repeat(MESH_TEXT_MAX_BYTES);
        let out = insert_mesh_snippet(&long, " more");
        assert!(out.len() <= MESH_TEXT_MAX_BYTES);
        assert_eq!(insert_mesh_snippet("", "hi"), "hi");
        assert_eq!(insert_mesh_snippet("hi", "there"), "hi there");
    }

    #[test]
    fn clamp_counts_utf8_bytes_not_scalars() {
        use super::{append_emoji_within, clamp_mesh_text_to};
        // U+1F525 FIRE is 4 UTF-8 bytes.
        let fire = "🔥";
        assert_eq!(fire.len(), 4);
        assert_eq!(fire.chars().count(), 1);
        let forty = fire.repeat(40); // 160 bytes, 40 scalars
        assert_eq!(forty.len(), 160);
        assert_eq!(clamp_mesh_text_to(format!("{forty}{fire}"), 160).len(), 160);
        assert_eq!(append_emoji_within(&forty, fire, 160), forty);
        assert_eq!(append_emoji_within("hi", fire, 160), format!("hi{fire}"));
    }
}
