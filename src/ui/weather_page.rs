//! Operator weather dashboard from the local station cache (Ecowitt / MQTT).

use dioxus::prelude::*;

use crate::services::{
    age_label, any_station_source_enabled, current_station_reading, outdoor_temp_display,
    station_is_fresh, station_label, station_stale_secs, weather_unit_f, StationReading,
};

use super::icons::{IconName};
use super::state;
use super::widgets::{Badge, PageHeading};

fn compass(deg: f64) -> &'static str {
    const DIRS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let idx = ((deg.rem_euclid(360.0) + 22.5) / 45.0) as usize % 8;
    DIRS[idx]
}

fn fmt_opt(n: Option<f64>, suffix: &str, decimals: usize) -> String {
    match n {
        Some(v) if decimals == 0 => format!("{v:.0}{suffix}"),
        Some(v) => format!("{v:.1}{suffix}"),
        None => "—".into(),
    }
}

fn wind_line(reading: &StationReading, fahrenheit: bool) -> String {
    let Some(spd) = reading.wind_mph else {
        return "—".into();
    };
    let speed = if fahrenheit {
        format!("{spd:.0} mph")
    } else {
        format!("{:.0} km/h", spd * 1.60934)
    };
    match reading.wind_dir_deg {
        Some(dir) => format!("{} {speed}", compass(dir)),
        None => speed,
    }
}

fn pressure_line(reading: &StationReading, fahrenheit: bool) -> String {
    if fahrenheit {
        if let Some(inhg) = reading.pressure_inhg {
            return format!("{inhg:.2} inHg");
        }
        if let Some(hpa) = reading.pressure_hpa {
            return format!("{:.2} inHg", hpa / 33.8639);
        }
    } else if let Some(hpa) = reading.pressure_hpa {
        return format!("{hpa:.0} hPa");
    } else if let Some(inhg) = reading.pressure_inhg {
        return format!("{:.0} hPa", inhg * 33.8639);
    }
    "—".into()
}

#[component]
pub fn WeatherPage() -> Element {
    let mut tick = use_signal(|| 0u64);
    use_effect(move || {
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                tick.with_mut(|n| *n = n.wrapping_add(1));
            }
        });
    });
    let _ = tick();

    let db = &state().db;
    let enabled = any_station_source_enabled(db);
    let fahrenheit = weather_unit_f(db);
    let label = station_label(db);
    let stale_secs = station_stale_secs(db);
    let reading = current_station_reading();
    let fresh = station_is_fresh(db);

    let outdoor = reading
        .as_ref()
        .and_then(|r| outdoor_temp_display(r, fahrenheit))
        .unwrap_or_else(|| "—".into());
    let humidity = reading
        .as_ref()
        .map(|r| fmt_opt(r.humidity_pct, "%", 0))
        .unwrap_or_else(|| "—".into());
    let feels = reading.as_ref().and_then(|r| {
        r.feels_like_f.map(|feel| {
            if fahrenheit {
                format!("Feels {feel:.0}F")
            } else {
                format!("Feels {:.0}C", (feel - 32.0) * 5.0 / 9.0)
            }
        })
    });
    let wind = reading
        .as_ref()
        .map(|r| wind_line(r, fahrenheit))
        .unwrap_or_else(|| "—".into());
    let gust = reading
        .as_ref()
        .map(|r| {
            let g = r.wind_gust_mph.map(|v| if fahrenheit { v } else { v * 1.60934 });
            let suf = if fahrenheit { " mph" } else { " km/h" };
            format!("Gust {}", fmt_opt(g, suf, 0))
        })
        .unwrap_or_default();
    let pressure = reading
        .as_ref()
        .map(|r| pressure_line(r, fahrenheit))
        .unwrap_or_else(|| "—".into());
    let rain_day = reading
        .as_ref()
        .map(|r| {
            if fahrenheit {
                fmt_opt(r.rain_day_in, " in today", 2)
            } else {
                fmt_opt(r.rain_day_in.map(|i| i * 25.4), " mm today", 1)
            }
        })
        .unwrap_or_else(|| "—".into());
    let rain_rate = reading
        .as_ref()
        .map(|r| {
            if fahrenheit {
                format!("Rate {}", fmt_opt(r.rain_rate_in, " in/h", 2))
            } else {
                format!(
                    "Rate {}",
                    fmt_opt(r.rain_rate_in.map(|i| i * 25.4), " mm/h", 1)
                )
            }
        })
        .unwrap_or_default();
    let uv = reading
        .as_ref()
        .map(|r| format!("UV {}", fmt_opt(r.uv_index, "", 0)))
        .unwrap_or_else(|| "UV —".into());
    let solar = reading
        .as_ref()
        .map(|r| format!("Solar {}", fmt_opt(r.solar_wm2, " W/m²", 0)))
        .unwrap_or_default();
    let indoor = reading
        .as_ref()
        .map(|r| {
            if fahrenheit {
                r.indoor_temp_f
                    .map(|t| format!("{t:.0}F"))
                    .unwrap_or_else(|| "—".into())
            } else {
                r.indoor_temp_f
                    .map(|f| format!("{:.0}C", (f - 32.0) * 5.0 / 9.0))
                    .unwrap_or_else(|| "—".into())
            }
        })
        .unwrap_or_else(|| "—".into());
    let indoor_hum = reading
        .as_ref()
        .map(|r| fmt_opt(r.indoor_humidity_pct, "%", 0))
        .unwrap_or_else(|| "—".into());
    let source_label = reading
        .as_ref()
        .map(|r| r.source.as_label().to_string())
        .unwrap_or_default();
    let age = reading
        .as_ref()
        .map(|r| age_label(r.received_at.elapsed().as_secs() as i64))
        .unwrap_or_default();
    let device = reading
        .as_ref()
        .map(|r| r.device.clone())
        .unwrap_or_default();

    rsx! {
        div { class: "hq-page-fill gap-4 p-1",
            PageHeading { icon: IconName::Weather, title: "Weather".to_string(),
                "Local station dashboard. Mesh #weather prefers a fresh reading; ZIP lookups stay on Open-Meteo."
            }

            if !enabled {
                div { class: "rounded-lg border border-border bg-card p-4",
                    p { class: "text-sm text-muted-foreground",
                        "No station source is enabled. Turn on Ecowitt push/pull or MQTT under Settings → HQ → Services."
                    }
                }
            } else if reading.is_none() {
                div { class: "rounded-lg border border-border bg-card p-4",
                    p { class: "text-sm text-muted-foreground",
                        "Waiting for the first station reading. For Ecowitt push, point Customized upload (Ecowitt protocol) at this HQ’s LAN IP. For pull, set the gateway IP. MQTT needs a broker topic."
                    }
                }
            } else {
                if !fresh {
                    div { class: "rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm text-amber-100",
                        "Reading is stale (>{stale_secs}s). Bare #weather falls back to Open-Meteo until a fresh observation arrives."
                    }
                }
                div { class: "flex flex-wrap items-center gap-2",
                    h3 { class: "text-lg font-semibold", "{label}" }
                    Badge { tone: if fresh { "signal".to_string() } else { "pending".to_string() },
                        "{source_label}"
                    }
                    span { class: "text-xs text-muted-foreground", "{age}" }
                    if !device.is_empty() {
                        span { class: "font-mono text-xs text-muted-foreground", "{device}" }
                    }
                }
                div { class: "grid gap-3 sm:grid-cols-2 lg:grid-cols-3",
                    div { class: "rounded-lg border border-border bg-card p-4",
                        p { class: "mb-1 text-xs uppercase tracking-[0.16em] text-muted-foreground", "Outdoor" }
                        p { class: "text-3xl font-semibold tabular-nums", "{outdoor}" }
                        p { class: "mt-1 text-sm text-muted-foreground", "Humidity {humidity}" }
                        if let Some(feels) = feels {
                            p { class: "text-xs text-muted-foreground", "{feels}" }
                        }
                    }
                    div { class: "rounded-lg border border-border bg-card p-4",
                        p { class: "mb-1 text-xs uppercase tracking-[0.16em] text-muted-foreground", "Wind" }
                        p { class: "text-xl font-semibold", "{wind}" }
                        p { class: "mt-1 text-sm text-muted-foreground", "{gust}" }
                    }
                    div { class: "rounded-lg border border-border bg-card p-4",
                        p { class: "mb-1 text-xs uppercase tracking-[0.16em] text-muted-foreground", "Pressure" }
                        p { class: "text-xl font-semibold", "{pressure}" }
                    }
                    div { class: "rounded-lg border border-border bg-card p-4",
                        p { class: "mb-1 text-xs uppercase tracking-[0.16em] text-muted-foreground", "Rain" }
                        p { class: "text-xl font-semibold", "{rain_day}" }
                        p { class: "mt-1 text-sm text-muted-foreground", "{rain_rate}" }
                    }
                    div { class: "rounded-lg border border-border bg-card p-4",
                        p { class: "mb-1 text-xs uppercase tracking-[0.16em] text-muted-foreground", "Sun" }
                        p { class: "text-xl font-semibold", "{uv}" }
                        p { class: "mt-1 text-sm text-muted-foreground", "{solar}" }
                    }
                    div { class: "rounded-lg border border-border bg-card p-4",
                        p { class: "mb-1 text-xs uppercase tracking-[0.16em] text-muted-foreground", "Indoor" }
                        p { class: "text-xl font-semibold tabular-nums", "{indoor}" }
                        p { class: "mt-1 text-sm text-muted-foreground", "Humidity {indoor_hum}" }
                    }
                }
            }
        }
    }
}
