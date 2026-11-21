//! Shared persistence helpers: timestamp conversion and error mapping.

use chrono::{DateTime, Utc};
use portos_core::{CoreError, CoreResult};
use rusqlite::Connection;

/// Serialize a timestamp for storage (RFC 3339, UTC).
#[must_use]
pub fn ts(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

/// Parse a stored timestamp.
pub fn parse_ts(s: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| CoreError::Internal(format!("bad timestamp {s:?}: {e}")))
}

pub fn parse_ts_opt(s: Option<String>) -> CoreResult<Option<DateTime<Utc>>> {
    match s {
        Some(s) => Ok(Some(parse_ts(&s)?)),
        None => Ok(None),
    }
}

/// Map a rusqlite error, translating constraint violations to conflicts.
pub fn map_err(e: rusqlite::Error) -> CoreError {
    if matches!(
        e,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error { code: rusqlite::ffi::ErrorCode::ConstraintViolation, .. },
            _,
        )
    ) {
        CoreError::Conflict(e.to_string())
    } else {
        CoreError::Internal(format!("db: {e}"))
    }
}

/// Run a function on a pooled connection (blocking; the API wraps calls in
/// `spawn_blocking`). Mutable access so callers can open transactions.
pub fn on_conn<T>(
    conn: &std::sync::Arc<std::sync::Mutex<Connection>>,
    f: impl FnOnce(&mut Connection) -> CoreResult<T>,
) -> CoreResult<T> {
    let mut guard = conn
        .lock()
        .map_err(|_| CoreError::Internal("db mutex poisoned".into()))?;
    f(&mut guard)
}

/// Serialize a serde enum as its bare snake_case tag (no JSON quotes) so
/// SQL literals can compare against it (`status = 'in_progress'`).
pub fn enum_str<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v)
        .map(|s| s.trim_matches('"').to_string())
        .unwrap_or_default()
}

/// Wrap a bare tag back into JSON for parsing.
#[macro_export]
macro_rules! parse_enum {
    ($t:ty, $s:expr) => {
        serde_json::from_str::<$t>(&format!("\"{}\"", $s))
    };
}
