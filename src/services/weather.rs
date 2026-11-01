//! Open-Meteo current + forecast → compact or multi-frame mesh replies.
//!
//! Data source is HQ-side only — do not stamp "Open-Meteo" on every airtime line.

use serde::Deserialize;

use crate::signup::{fit_mesh, mesh_frames};

#[derive(Debug, Clone)]
pub struct WeatherQuery {
    pub lat: f64,
    pub lon: f64,
    pub label: String,
    pub fahrenheit: bool,
}

#[derive(Debug, Deserialize)]
struct ForecastResponse {
    current: Option<CurrentBlock>,
    daily: Option<DailyBlock>,
}

#[derive(Debug, Deserialize)]
struct CurrentBlock {
    temperature_2m: Option<f64>,
    relative_humidity_2m: Option<f64>,
    apparent_temperature: Option<f64>,
    weather_code: Option<i64>,
    wind_speed_10m: Option<f64>,
    wind_direction_10m: Option<f64>,
    precipitation: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct DailyBlock {
    time: Option<Vec<String>>,
    weather_code: Option<Vec<i64>>,
    temperature_2m_max: Option<Vec<f64>>,
    temperature_2m_min: Option<Vec<f64>>,
    precipitation_sum: Option<Vec<f64>>,
}

/// Blocking HTTP (call from `spawn_blocking`). Compact one-liner.
pub fn fetch_weather_line(query: &WeatherQuery) -> Result<String, String> {
    let data = fetch_forecast(query, 2)?;
    Ok(format_forecast(query, &data))
}

/// Compact current line plus 1–2 follow-up frames (feels-like / multi-day).
pub fn fetch_weather_detail(query: &WeatherQuery) -> Result<Vec<String>, String> {
    let data = fetch_forecast(query, 4)?;
    Ok(format_forecast_detail(query, &data))
}

fn fetch_forecast(query: &WeatherQuery, forecast_days: u8) -> Result<ForecastResponse, String> {
    let temp_unit = if query.fahrenheit {
        "fahrenheit"
    } else {
        "celsius"
    };
    let wind_unit = if query.fahrenheit { "mph" } else { "kmh" };
    let days = forecast_days.clamp(2, 4);
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.4}&longitude={:.4}\
         &current=temperature_2m,relative_humidity_2m,apparent_temperature,weather_code,\
         wind_speed_10m,wind_direction_10m,precipitation\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_sum\
         &temperature_unit={temp_unit}&wind_speed_unit={wind_unit}\
         &timezone=auto&forecast_days={days}",
        query.lat, query.lon
    );
    let body = crate::services::http_get(&url)
        .map_err(|e| format!("Weather fetch failed: {e}"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&body).map_err(|e| format!("Weather parse error: {e}"))
}

/// Turn Nominatim / free-text into a short mesh label (`Hesperia` or `Hesperia 92345`).
///
/// Nominatim often returns `92345, Hesperia, County, …` — never truncate that raw
/// string; skip numeric segments and keep the city name intact.
pub fn short_wx_label(raw: &str, zip_hint: Option<&str>) -> String {
    let zip = zip_hint
        .map(str::trim)
        .filter(|z| !z.is_empty())
        .map(|z| z.to_string());

    let city = raw
        .split(',')
        .map(str::trim)
        .find(|part| {
            !part.is_empty()
                && !part
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '-' || c.is_whitespace())
        })
        .unwrap_or("")
        .to_string();

    // Keep full city words when possible (avoid "Hesperi" / "Alta Lo").
    let city = if city.chars().count() > 18 {
        // Prefer cutting on a space rather than mid-word.
        let mut cut = 18;
        let chars: Vec<char> = city.chars().collect();
        while cut > 8 && chars.get(cut).is_some_and(|c| *c != ' ') {
            cut -= 1;
        }
        if chars.get(cut).is_some_and(|c| *c == ' ') {
            chars[..cut].iter().collect()
        } else {
            chars.into_iter().take(18).collect()
        }
    } else {
        city
    };

    match (city.as_str(), zip.as_deref()) {
        ("", Some(zip)) => zip.to_string(),
        ("", None) => "HQ".into(),
        (city, Some(zip)) if !city.eq_ignore_ascii_case(zip) => {
            let combined = format!("{city} {zip}");
            if combined.chars().count() <= 24 {
                combined
            } else {
                city.to_string()
            }
        }
        (city, _) => city.to_string(),
    }
}

fn format_forecast(query: &WeatherQuery, data: &ForecastResponse) -> String {
    let label = short_wx_label(&query.label, None);
    let now = current_brief(query, data);
    let tomorrow = daily_brief(query, data, 1).unwrap_or_else(|| "n/a".into());
    fit_mesh(&format!("WX {label}: {now} | Tom {tomorrow}"))
}

fn format_forecast_detail(query: &WeatherQuery, data: &ForecastResponse) -> Vec<String> {
    let label = short_wx_label(&query.label, None);
    let unit = if query.fahrenheit { "F" } else { "C" };
    let wind_u = if query.fahrenheit { "mph" } else { "kmh" };
    let current = data.current.as_ref();

    let mut frames = Vec::new();
    frames.push(format_forecast(query, data));

    // Feels-like / precip extras when they add signal.
    let mut extras: Vec<String> = Vec::new();
    if let Some(feel) = current.and_then(|c| c.apparent_temperature) {
        extras.push(format!("feels {feel:.0}{unit}"));
    }
    if let Some(precip) = current.and_then(|c| c.precipitation).filter(|p| *p > 0.05) {
        let p_u = if query.fahrenheit { "in" } else { "mm" };
        extras.push(format!("precip {precip:.2}{p_u}"));
    }
    if let Some(spd) = current.and_then(|c| c.wind_speed_10m) {
        if let Some(dir) = current.and_then(|c| c.wind_direction_10m) {
            extras.push(format!("wind {}{spd:.0}{wind_u}", compass(dir)));
        }
    }
    if !extras.is_empty() {
        frames.extend(mesh_frames("WX+", &format!("{label}: {}", extras.join(" · ")), 2));
    }

    // Day-by-day outlook (skip index 0 if already in compact "now"; include today hi/lo + next days).
    let mut outlook: Vec<String> = Vec::new();
    for day in 0..4 {
        if let Some(line) = daily_brief(query, data, day) {
            let tag = match day {
                0 => "Today",
                1 => "Tom",
                2 => "Day+2",
                _ => "Day+3",
            };
            outlook.push(format!("{tag} {line}"));
        }
    }
    if !outlook.is_empty() {
        frames.extend(mesh_frames("WX+", &outlook.join(" · "), 3));
    }

    // Cap total airtime.
    frames.truncate(5);
    frames
}

fn current_brief(query: &WeatherQuery, data: &ForecastResponse) -> String {
    let unit = if query.fahrenheit { "F" } else { "C" };
    let wind_u = if query.fahrenheit { "mph" } else { "kmh" };
    let current = data.current.as_ref();
    let code = current.and_then(|c| c.weather_code).unwrap_or(-1);
    let temp = current
        .and_then(|c| c.temperature_2m)
        .map(|t| format!("{t:.0}{unit}"))
        .unwrap_or_else(|| "?".into());
    let rh = current
        .and_then(|c| c.relative_humidity_2m)
        .map(|h| format!(" {h:.0}%"))
        .unwrap_or_default();
    let wind = match (
        current.and_then(|c| c.wind_speed_10m),
        current.and_then(|c| c.wind_direction_10m),
    ) {
        (Some(spd), Some(dir)) => format!(" {}{:.0}{wind_u}", compass(dir), spd),
        (Some(spd), None) => format!(" {spd:.0}{wind_u}"),
        _ => String::new(),
    };
    format!("{} {temp}{rh}{wind}", wmo_short(code))
}

fn daily_brief(query: &WeatherQuery, data: &ForecastResponse, idx: usize) -> Option<String> {
    let unit = if query.fahrenheit { "F" } else { "C" };
    let d = data.daily.as_ref()?;
    let code = d.weather_code.as_ref()?.get(idx).copied().unwrap_or(-1);
    let hi = d.temperature_2m_max.as_ref()?.get(idx).copied();
    let lo = d.temperature_2m_min.as_ref()?.get(idx).copied();
    let precip = d
        .precipitation_sum
        .as_ref()
        .and_then(|v| v.get(idx).copied())
        .filter(|p| *p > 0.05);
    let range = match (lo, hi) {
        (Some(lo), Some(hi)) => format!("{lo:.0}-{hi:.0}{unit}"),
        _ => return None,
    };
    let mut line = format!("{} {range}", wmo_short(code));
    if let Some(p) = precip {
        let p_u = if query.fahrenheit { "in" } else { "mm" };
        line.push_str(&format!(" {p:.1}{p_u}"));
    }
    // Silence unused `time` field warning by acknowledging it exists for future weekday labels.
    let _ = d.time.as_ref().and_then(|t| t.get(idx));
    Some(line)
}

fn compass(deg: f64) -> &'static str {
    const DIRS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let idx = ((deg.rem_euclid(360.0) + 22.5) / 45.0) as usize % 8;
    DIRS[idx]
}

fn wmo_short(code: i64) -> &'static str {
    match code {
        0 => "Clear",
        1 | 2 => "Fair",
        3 => "Cloudy",
        45 | 48 => "Fog",
        51 | 53 | 55 | 56 | 57 => "Drizzle",
        61 | 63 | 65 | 66 | 67 => "Rain",
        71 | 73 | 75 | 77 => "Snow",
        80 | 81 | 82 => "Showers",
        85 | 86 => "SnowShwr",
        95 => "TStorm",
        96 | 99 => "TStormHail",
        _ => "Wx",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_data() -> ForecastResponse {
        ForecastResponse {
            current: Some(CurrentBlock {
                temperature_2m: Some(58.0),
                relative_humidity_2m: Some(40.0),
                apparent_temperature: Some(55.0),
                weather_code: Some(1),
                wind_speed_10m: Some(16.0),
                wind_direction_10m: Some(225.0),
                precipitation: Some(0.0),
            }),
            daily: Some(DailyBlock {
                time: Some(vec![
                    "2026-11-01".into(),
                    "2026-11-02".into(),
                    "2026-11-03".into(),
                ]),
                weather_code: Some(vec![1, 3, 61]),
                temperature_2m_max: Some(vec![65.0, 65.0, 58.0]),
                temperature_2m_min: Some(vec![41.0, 41.0, 44.0]),
                precipitation_sum: Some(vec![0.0, 0.0, 0.4]),
            }),
        }
    }

    #[test]
    fn formats_compact_line() {
        let query = WeatherQuery {
            lat: 34.0,
            lon: -118.0,
            label: short_wx_label(
                "92345, Hesperia, San Bernardino County, California, United States",
                Some("92345"),
            ),
            fahrenheit: true,
        };
        assert_eq!(query.label, "Hesperia 92345");
        let line = format_forecast(&query, &sample_data());
        assert!(line.len() <= 160, "{line}");
        assert!(line.starts_with("WX Hesperia 92345:"), "{line}");
        assert!(!line.contains("Hesperi:"), "{line}");
        assert!(!line.contains("92345, "), "{line}");
        assert!(line.contains("58F"));
        assert!(!line.contains("Open-Meteo"), "{line}");
    }

    #[test]
    fn detail_spans_multiple_frames() {
        let query = WeatherQuery {
            lat: 34.0,
            lon: -118.0,
            label: "Hesperia 92345".into(),
            fahrenheit: true,
        };
        let frames = format_forecast_detail(&query, &sample_data());
        assert!(frames.len() >= 2, "{frames:?}");
        assert!(frames.iter().all(|f| f.len() <= 160));
        assert!(frames[0].starts_with("WX "));
        assert!(frames.iter().any(|f| f.contains("Today") || f.contains("Day+2")));
    }

    #[test]
    fn alta_loma_not_chopped() {
        assert_eq!(
            short_wx_label(
                "91737, Alta Loma, San Bernardino County, California, United States",
                Some("91737"),
            ),
            "Alta Loma 91737"
        );
    }
}
