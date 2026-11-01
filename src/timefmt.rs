//! Local date/time stamps using Settings → time_format / date_format.
//!
//! Always applies this computer’s local timezone (same as the Messages UI).

use chrono::{Local, TimeZone};

use crate::db::Db;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeFormatPref {
    TwelveHour,
    TwentyFourHour,
}

impl TimeFormatPref {
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "24h" | "24" => Self::TwentyFourHour,
            _ => Self::TwelveHour,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::TwelveHour => "12h",
            Self::TwentyFourHour => "24h",
        }
    }

    pub fn chrono_clock(self) -> &'static str {
        match self {
            Self::TwelveHour => "%-I:%M %p",
            Self::TwentyFourHour => "%H:%M",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateFormatPref {
    /// Oct 31
    MonthDayShort,
    /// Oct 31, 2026
    MonthDayYear,
    /// 10/31/2026
    MdyNumeric,
    /// 31/10/2026
    DmyNumeric,
    /// 2026-10-31
    YmdIso,
}

impl DateFormatPref {
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "mdy_long" | "month_day_year" => Self::MonthDayYear,
            "mdy" | "mdy_numeric" => Self::MdyNumeric,
            "dmy" | "dmy_numeric" => Self::DmyNumeric,
            "ymd" | "iso" => Self::YmdIso,
            _ => Self::MonthDayShort,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MonthDayShort => "md_short",
            Self::MonthDayYear => "mdy_long",
            Self::MdyNumeric => "mdy",
            Self::DmyNumeric => "dmy",
            Self::YmdIso => "ymd",
        }
    }

    pub fn chrono_date(self) -> &'static str {
        // %-d / %-m drop leading zeros on Unix; Local already applies the computer TZ.
        match self {
            Self::MonthDayShort => "%b %-d",
            Self::MonthDayYear => "%b %-d, %Y",
            Self::MdyNumeric => "%-m/%-d/%Y",
            Self::DmyNumeric => "%-d/%-m/%Y",
            Self::YmdIso => "%Y-%m-%d",
        }
    }
}

pub fn load_time_format(db: &Db) -> TimeFormatPref {
    db.setting("time_format")
        .ok()
        .flatten()
        .map(|value| TimeFormatPref::parse(&value))
        .unwrap_or(TimeFormatPref::TwelveHour)
}

pub fn load_date_format(db: &Db) -> DateFormatPref {
    db.setting("date_format")
        .ok()
        .flatten()
        .map(|value| DateFormatPref::parse(&value))
        .unwrap_or(DateFormatPref::MonthDayShort)
}

/// Date + clock in local TZ, e.g. `Nov 1 8:46 AM`.
pub fn format_stamp(db: &Db, unix_secs: i64) -> String {
    let Some(time) = Local.timestamp_opt(unix_secs, 0).single() else {
        return unix_secs.to_string();
    };
    let date = load_date_format(db).chrono_date();
    let clock = load_time_format(db).chrono_clock();
    time.format(&format!("{date} {clock}")).to_string()
}

/// Clock only in local TZ, e.g. `8:46 AM`.
pub fn format_clock(db: &Db, unix_secs: i64) -> String {
    let Some(time) = Local.timestamp_opt(unix_secs, 0).single() else {
        return unix_secs.to_string();
    };
    time.format(load_time_format(db).chrono_clock()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn stamp_is_local_not_zulu() {
        let db = Db::from_conn(Connection::open_in_memory().unwrap()).unwrap();
        db.ensure_defaults().unwrap();
        let _ = db.set_setting("time_format", "12h");
        let _ = db.set_setting("date_format", "md_short");
        // Fixed UTC instant — output must not use Zulu suffix.
        let stamp = format_stamp(&db, 1_730_468_760);
        assert!(!stamp.contains('Z'), "{stamp}");
        assert!(stamp.contains(':'), "{stamp}");
        assert!(
            stamp.contains("AM") || stamp.contains("PM"),
            "{stamp}"
        );
    }

    #[test]
    fn twenty_four_hour_has_no_ampm() {
        let db = Db::from_conn(Connection::open_in_memory().unwrap()).unwrap();
        db.ensure_defaults().unwrap();
        let _ = db.set_setting("time_format", "24h");
        let _ = db.set_setting("date_format", "ymd");
        let stamp = format_stamp(&db, 1_730_468_760);
        assert!(!stamp.contains("AM") && !stamp.contains("PM"), "{stamp}");
        assert!(stamp.starts_with("20"), "{stamp}"); // ymd
    }
}
