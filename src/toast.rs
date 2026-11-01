//! Transient toast notifications for operator feedback (not the security audit log).

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const CAPACITY: usize = 6;
const DEFAULT_TTL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warn,
    Error,
}

impl ToastKind {
    pub fn class(self) -> &'static str {
        match self {
            Self::Info => "toast toast-info",
            Self::Success => "toast toast-success",
            Self::Warn => "toast toast-warn",
            Self::Error => "toast toast-error",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub message: String,
    expires_at: Instant,
}

struct ToastStore {
    next_id: u64,
    items: VecDeque<Toast>,
}

fn store() -> &'static Mutex<ToastStore> {
    static STORE: OnceLock<Mutex<ToastStore>> = OnceLock::new();
    STORE.get_or_init(|| {
        Mutex::new(ToastStore {
            next_id: 0,
            items: VecDeque::new(),
        })
    })
}

pub fn push(kind: ToastKind, message: impl Into<String>) {
    push_for(kind, message, DEFAULT_TTL);
}

pub fn push_for(kind: ToastKind, message: impl Into<String>, ttl: Duration) {
    let message = message.into();
    let message = crate::app_log::redact(&message);
    // Mirror into the diagnostics log so toasts are searchable later.
    match kind {
        ToastKind::Error => hq_log!(error, "{message}"),
        ToastKind::Warn => hq_log!(warn, "{message}"),
        ToastKind::Success | ToastKind::Info => hq_log!(info, "{message}"),
    }
    let mut guard = match store().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let id = guard.next_id.wrapping_add(1);
    guard.next_id = id;
    guard.items.push_back(Toast {
        id,
        kind,
        message,
        expires_at: Instant::now() + ttl,
    });
    while guard.items.len() > CAPACITY {
        guard.items.pop_front();
    }
}

pub fn dismiss(id: u64) {
    let mut guard = match store().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.items.retain(|t| t.id != id);
}

pub fn prune_expired() {
    let now = Instant::now();
    let mut guard = match store().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.items.retain(|t| t.expires_at > now);
}

pub fn snapshot() -> Vec<Toast> {
    prune_expired();
    let guard = match store().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard.items.iter().cloned().collect()
}

/// Infer toast kind from common message prefixes.
pub fn infer_kind(message: &str) -> ToastKind {
    let lower = message.to_ascii_lowercase();
    if lower.contains("fail")
        || lower.contains("error")
        || lower.contains("could not")
        || lower.contains("did not finish")
    {
        ToastKind::Error
    } else if lower.contains("deleted")
        || lower.contains("created")
        || lower.contains("cleared")
        || lower.contains("applied")
        || lower.contains("is now")
        || lower.contains("keep →")
        || lower.contains("keep ->")
    {
        ToastKind::Success
    } else if lower.contains("clearing") || lower.contains("connect the companion") {
        ToastKind::Warn
    } else {
        ToastKind::Info
    }
}
