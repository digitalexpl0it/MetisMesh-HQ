//! In-app diagnostic log (ring buffer), mirrored to `log_file` when enabled.
//! Separate from the security audit table.
//! Never store channel secrets, PINs, or key material in clear text.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const CAPACITY: usize = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error = 1,
    Warn = 2,
    Info = 3,
    Debug = 4,
    /// Verbose / trace — noisiest.
    Verbose = 5,
}

impl LogLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Verbose => "verbose",
        }
    }
}

/// Filter for the log viewer dropdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFilter {
    All,
    Error,
    Warn,
    Info,
    Debug,
    Verbose,
}

impl LogFilter {
    pub fn from_value(value: &str) -> Self {
        match value {
            "error" => Self::Error,
            "warn" => Self::Warn,
            "info" => Self::Info,
            "debug" => Self::Debug,
            "verbose" => Self::Verbose,
            _ => Self::All,
        }
    }

    pub fn value(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
            Self::Verbose => "verbose",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Error => "Error",
            Self::Warn => "Warn",
            Self::Info => "Info",
            Self::Debug => "Debug",
            Self::Verbose => "Verbose",
        }
    }

    /// Threshold filter: show this level and more severe (Verbose = everything).
    fn allows(self, level: LogLevel) -> bool {
        match self {
            Self::All | Self::Verbose => true,
            Self::Error => level == LogLevel::Error,
            Self::Warn => level <= LogLevel::Warn,
            Self::Info => level <= LogLevel::Info,
            Self::Debug => level <= LogLevel::Debug,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub id: u64,
    pub at: i64,
    pub level: LogLevel,
    pub message: String,
}

fn store() -> &'static Mutex<LogStore> {
    static STORE: OnceLock<Mutex<LogStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(LogStore::default()))
}

#[derive(Default)]
struct LogStore {
    next_id: u64,
    entries: VecDeque<LogEntry>,
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Scrub obvious secrets before a line is stored or printed.
pub fn redact(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let sensitive = [
        "secret",
        "password",
        "passwd",
        " pin",
        "pin=",
        "pin:",
        "private key",
        "prv.key",
        "wifi.pwd",
    ];
    if sensitive.iter().any(|needle| lower.contains(needle)) {
        // Keep a short prefix so the event type is still readable.
        let cut = text
            .char_indices()
            .nth(48)
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        return format!("{}… [redacted]", &text[..cut]);
    }
    text.to_string()
}

pub fn push(level: LogLevel, message: impl Into<String>) {
    let message = redact(&message.into());
    // Mirror to stderr so `dx serve` / journal still show the line.
    eprintln!("[{}] {message}", level.as_str());
    // Before taking the buffer lock: the file writer may read the buffer.
    crate::log_file::write_line(level, &message);
    let mut guard = match store().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let id = guard.next_id.wrapping_add(1);
    guard.next_id = id;
    guard.entries.push_back(LogEntry {
        id,
        at: unix_now(),
        level,
        message,
    });
    while guard.entries.len() > CAPACITY {
        guard.entries.pop_front();
    }
}

pub fn clear() {
    let mut guard = match store().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.entries.clear();
}

pub fn snapshot() -> Vec<LogEntry> {
    let guard = match store().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.entries.iter().cloned().collect()
}

pub fn filtered(filter: LogFilter, query: &str) -> Vec<LogEntry> {
    let mut rows: Vec<(i32, LogEntry)> = snapshot()
        .into_iter()
        .filter(|e| filter.allows(e.level))
        .filter_map(|e| {
            let score = fuzzy_score(query, &e.message)?;
            Some((score, e))
        })
        .collect();
    if !query.trim().is_empty() {
        // Better fuzzy matches first; stable by id within a score.
        rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.id.cmp(&b.1.id)));
    } else {
        // Newest last so the transcript reads top→bottom like a console.
        rows.sort_by_key(|(_, e)| e.id);
    }
    rows.into_iter().map(|(_, e)| e).collect()
}

/// Case-insensitive contains (best) or subsequence fuzzy match.
/// Returns a score, or `None` if no match.
fn fuzzy_score(query: &str, haystack: &str) -> Option<i32> {
    let q = query.trim();
    if q.is_empty() {
        return Some(0);
    }
    let q_l = q.to_ascii_lowercase();
    let h_l = haystack.to_ascii_lowercase();
    if h_l.contains(&q_l) {
        return Some(1_000 - (h_l.find(&q_l).unwrap_or(0) as i32).min(900));
    }
    // Subsequence: every query char appears in order.
    let mut score = 100;
    let mut chars = h_l.chars();
    for qc in q_l.chars() {
        let mut found = false;
        while let Some(hc) = chars.next() {
            if hc == qc {
                found = true;
                break;
            }
            score -= 1;
        }
        if !found {
            return None;
        }
    }
    Some(score.max(1))
}

pub fn format_time(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|dt| dt.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--:--:--".into())
}

/// Most verbose library level that reaches the app log. meshcore-rs debug
/// dumps raw BLE frames (channel secrets in hex), so nothing below info.
fn library_max_level(target: &str) -> LogLevel {
    if target.starts_with("meshcore_rs") || target.starts_with("btleplug") {
        LogLevel::Info
    } else {
        LogLevel::Warn
    }
}

fn target_root(target: &str) -> &str {
    target.split("::").next().unwrap_or(target)
}

struct TracingBridge;

#[derive(Default)]
struct EventText {
    message: String,
    fields: String,
}

impl tracing::field::Visit for EventText {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            self.fields.push_str(&format!(" {}={value}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message.push_str(&format!("{value:?}"));
        } else {
            self.fields.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}

fn from_tracing_level(level: tracing::Level) -> LogLevel {
    match level {
        tracing::Level::ERROR => LogLevel::Error,
        tracing::Level::WARN => LogLevel::Warn,
        tracing::Level::INFO => LogLevel::Info,
        tracing::Level::DEBUG => LogLevel::Debug,
        tracing::Level::TRACE => LogLevel::Verbose,
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for TracingBridge {
    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        from_tracing_level(*metadata.level()) <= library_max_level(metadata.target())
    }

    fn max_level_hint(&self) -> Option<tracing::level_filters::LevelFilter> {
        Some(tracing::level_filters::LevelFilter::INFO)
    }

    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let meta = event.metadata();
        let mut text = EventText::default();
        event.record(&mut text);
        push(
            from_tracing_level(*meta.level()),
            format!("{}: {}{}", target_root(meta.target()), text.message, text.fields),
        );
    }
}

struct LogBridge;

fn from_log_level(level: log::Level) -> LogLevel {
    match level {
        log::Level::Error => LogLevel::Error,
        log::Level::Warn => LogLevel::Warn,
        log::Level::Info => LogLevel::Info,
        log::Level::Debug => LogLevel::Debug,
        log::Level::Trace => LogLevel::Verbose,
    }
}

impl log::Log for LogBridge {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        from_log_level(metadata.level()) <= library_max_level(metadata.target())
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            push(
                from_log_level(record.level()),
                format!("{}: {}", target_root(record.target()), record.args()),
            );
        }
    }

    fn flush(&self) {}
}

static LOG_BRIDGE: LogBridge = LogBridge;

/// Route meshcore-rs (`tracing`) and btleplug (`log`) lines into this log.
/// Call before Dioxus launch: Dioxus only installs its stdout logger when no
/// global `tracing` subscriber exists yet.
pub fn install_library_bridge() {
    use tracing_subscriber::layer::SubscriberExt;
    let subscriber = tracing_subscriber::registry().with(TracingBridge);
    if tracing::subscriber::set_global_default(subscriber).is_err() {
        push(LogLevel::Warn, "Library tracing bridge not installed (another logger is active).");
    }
    if log::set_logger(&LOG_BRIDGE).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    } else {
        push(LogLevel::Warn, "Library log bridge not installed (another logger is active).");
    }
}

macro_rules! hq_log {
    (error, $($arg:tt)*) => {
        $crate::app_log::push($crate::app_log::LogLevel::Error, format!($($arg)*))
    };
    (warn, $($arg:tt)*) => {
        $crate::app_log::push($crate::app_log::LogLevel::Warn, format!($($arg)*))
    };
    (info, $($arg:tt)*) => {
        $crate::app_log::push($crate::app_log::LogLevel::Info, format!($($arg)*))
    };
    (debug, $($arg:tt)*) => {
        $crate::app_log::push($crate::app_log::LogLevel::Debug, format!($($arg)*))
    };
    (verbose, $($arg:tt)*) => {
        $crate::app_log::push($crate::app_log::LogLevel::Verbose, format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_contains_and_subsequence() {
        assert!(fuzzy_score("send", "channel send: ok").is_some());
        assert!(fuzzy_score("chnsnd", "channel send: ok").is_some());
        assert!(fuzzy_score("zzz", "channel send: ok").is_none());
    }

    #[test]
    fn redact_secrets() {
        let out = redact("channel secret abcdef");
        assert!(out.contains("[redacted]"));
    }

    #[test]
    fn library_levels_never_pass_debug() {
        assert_eq!(library_max_level("meshcore_rs::meshcore::ble"), LogLevel::Info);
        assert_eq!(library_max_level("btleplug::winrtble"), LogLevel::Info);
        assert_eq!(library_max_level("wry::webview"), LogLevel::Warn);
        assert_eq!(target_root("meshcore_rs::reader"), "meshcore_rs");
        assert_eq!(target_root("tao"), "tao");
    }

    #[test]
    fn filter_threshold() {
        assert!(LogFilter::Warn.allows(LogLevel::Error));
        assert!(LogFilter::Warn.allows(LogLevel::Warn));
        assert!(!LogFilter::Warn.allows(LogLevel::Info));
        assert!(LogFilter::Error.allows(LogLevel::Error));
        assert!(!LogFilter::Error.allows(LogLevel::Warn));
    }
}
