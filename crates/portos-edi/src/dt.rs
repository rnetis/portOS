//! EDIFACT date/time helpers (format qualifier 203 = `CCYYMMDDHHMM`).

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum DtError {
    #[error("invalid EDIFACT timestamp {0:?} (expected CCYYMMDDHHMM)")]
    Invalid(String),
}

/// Parse `CCYYMMDDHHMM` (qualifier 203) as UTC.
pub fn parse_203(s: &str) -> Result<DateTime<Utc>, DtError> {
    let t = s.trim();
    if t.len() != 12 || !t.chars().all(|c| c.is_ascii_digit()) {
        return Err(DtError::Invalid(s.to_string()));
    }
    let fmt = NaiveDateTime::parse_from_str(t, "%Y%m%d%H%M")
        .map_err(|_| DtError::Invalid(s.to_string()))?;
    Ok(Utc.from_utc_datetime(&fmt))
}

/// Serialize to `CCYYMMDDHHMM` (UTC).
#[must_use]
pub fn emit_203(dt: DateTime<Utc>) -> String {
    dt.format("%Y%m%d%H%M").to_string()
}

/// Serialize to `CCYYMMDD` (qualifier 102).
#[must_use]
pub fn emit_102(dt: DateTime<Utc>) -> String {
    dt.format("%Y%m%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dt = parse_203("202611210930").unwrap();
        assert_eq!(emit_203(dt), "202611210930");
        assert_eq!(dt.to_rfc3339(), "2026-11-21T09:30:00+00:00");
        assert!(parse_203("2026112").is_err());
        assert!(parse_203("20261121993O").is_err());
    }
}
