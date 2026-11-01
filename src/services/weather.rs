//! Open-Meteo current + tomorrow forecast → one mesh line.

use serde::Deserialize;

use crate::signup::fit_mesh;

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
    weather_code: Option<i64>,
    wind_speed_10m: Option<f64>,
    wind_direction_10m: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct DailyBlock {
    weather_code: Option<Vec<i64>>,
    temperature_2m_max: Option<Vec<f64>>,
    temperature_2m_min: Option<Vec<f64>>,
}

/// Blocking HTTP (call from `spawn_blocking`).
pub fn fetch_weather_line(query: &WeatherQuery) -> Result<String, String> {
    let temp_unit = if query.fahrenheit {
        "fahrenheit"
    } else {
        "celsius"
    };
    let wind_unit = if query.fahrenheit { "mph" } else { "kmh" };
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.4}&longitude={:.4}\
         &current=temperature_2m,weather_code,wind_speed_10m,wind_direction_10m\
         &daily=weather_code,temperature_2m_max,temperature_2m_min\
         &temperature_unit={temp_unit}&wind_speed_unit={wind_unit}\
         &timezone=auto&forecast_days=2",
        query.lat, query.lon
    );
    let body = crate::services::http_get(&url)
        .map_err(|e| format!("Weather fetch failed: {e}"))?
        .into_string()
        .map_err(|e| e.to_string())?;
    let parsed: ForecastResponse =
        serde_json::from_str(&body).map_err(|e| format!("Weather parse error: {e}"))?;
    Ok(format_forecast(query, &parsed))
}

fn format_forecast(query: &WeatherQuery, data: &ForecastResponse) -> String {
    let unit = if query.fahrenheit { "F" } else { "C" };
    let wind_u = if query.fahrenheit { "mph" } else { "km/h" };
    let current = data.current.as_ref();
    let code = current.and_then(|c| c.weather_code).unwrap_or(-1);
    let temp = current
        .and_then(|c| c.temperature_2m)
        .map(|t| format!("{t:.0}{unit}"))
        .unwrap_or_else(|| "?".into());
    let wind = match (
        current.and_then(|c| c.wind_speed_10m),
        current.and_then(|c| c.wind_direction_10m),
    ) {
        (Some(spd), Some(dir)) => format!("{}{:.0}{wind_u}", compass(dir), spd),
        (Some(spd), None) => format!("{spd:.0}{wind_u}"),
        _ => String::new(),
    };
    let now = if wind.is_empty() {
        format!("{} {temp}", wmo_short(code))
    } else {
        format!("{} {temp} {wind}", wmo_short(code))
    };

    let tomorrow = data
        .daily
        .as_ref()
        .map(|d| {
            let code = d.weather_code.as_ref().and_then(|v| v.get(1)).copied().unwrap_or(-1);
            let hi = d
                .temperature_2m_max
                .as_ref()
                .and_then(|v| v.get(1))
                .copied();
            let lo = d
                .temperature_2m_min
                .as_ref()
                .and_then(|v| v.get(1))
                .copied();
            match (lo, hi) {
                (Some(lo), Some(hi)) => {
                    format!("{} {lo:.0}-{hi:.0}{unit}", wmo_short(code))
                }
                _ => wmo_short(code).to_string(),
            }
        })
        .unwrap_or_else(|| "n/a".into());

    let label = truncate_label(&query.label, 18);
    fit_mesh(&format!(
        "WX {label}: {now} | Tom: {tomorrow} · Open-Meteo"
    ))
}

fn truncate_label(label: &str, max_chars: usize) -> String {
    let trimmed = label.trim();
    if trimmed.is_empty() {
        return "HQ".into();
    }
    let count = trimmed.chars().count();
    if count <= max_chars {
        return trimmed.to_string();
    }
    trimmed.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "…"
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

    #[test]
    fn formats_compact_line() {
        let query = WeatherQuery {
            lat: 34.0,
            lon: -118.0,
            label: "Rose01-Hesperia".into(),
            fahrenheit: true,
        };
        let data = ForecastResponse {
            current: Some(CurrentBlock {
                temperature_2m: Some(68.4),
                weather_code: Some(0),
                wind_speed_10m: Some(8.2),
                wind_direction_10m: Some(20.0),
            }),
            daily: Some(DailyBlock {
                weather_code: Some(vec![0, 61]),
                temperature_2m_max: Some(vec![72.0, 72.0]),
                temperature_2m_min: Some(vec![55.0, 55.0]),
            }),
        };
        let line = format_forecast(&query, &data);
        assert!(line.len() <= 160);
        assert!(line.contains("Open-Meteo"));
        assert!(line.contains("Clear"));
        assert!(line.contains("Rain"));
    }
}
