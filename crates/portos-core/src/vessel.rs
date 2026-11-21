//! Vessels and vessel calls (PRD F-1, F-3, F-5, F-6).

use crate::error::{CoreError, CoreResult};
use crate::ids::{CallId, CustomerId, VesselId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// IMO ship identification number: 6 digits + check digit
/// (weights 7,6,5,4,3,2 from the left).
pub fn imo_check_digit(first6: &str) -> CoreResult<u8> {
    let bytes = first6.as_bytes();
    if bytes.len() != 6 || !bytes.iter().all(u8::is_ascii_digit) {
        return Err(CoreError::Validation("IMO base must be 6 digits".into()));
    }
    let mut sum: u32 = 0;
    for (i, &c) in bytes.iter().enumerate() {
        sum += u32::from(c - b'0') * (7 - i as u32);
    }
    Ok((sum % 10) as u8)
}

/// A validated IMO number.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ImoNumber(String);

impl ImoNumber {
    pub fn parse(s: &str) -> CoreResult<ImoNumber> {
        let raw = s.trim().replace([' ', '-'], "");
        let bytes = raw.as_bytes();
        if bytes.len() != 7 || !bytes.iter().all(u8::is_ascii_digit) {
            return Err(CoreError::Validation(format!(
                "IMO number must be 7 digits: {raw:?}"
            )));
        }
        let expected = imo_check_digit(&raw[..6])?;
        let actual: u8 = raw[6..]
            .parse()
            .map_err(|_| CoreError::Validation("IMO check digit not a number".into()))?;
        if expected != actual {
            return Err(CoreError::Validation(format!(
                "invalid IMO check digit: expected {expected}, got {actual} ({raw})"
            )));
        }
        Ok(ImoNumber(raw))
    }

    /// Build a valid IMO number from its first six digits.
    pub fn from_prefix(first6: &str) -> CoreResult<ImoNumber> {
        let check = imo_check_digit(first6)?;
        ImoNumber::parse(&format!("{first6}{check}"))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ImoNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ImoNumber {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ImoNumber::parse(s)
    }
}

/// UN/LOCODE (5 chars: country + location), kept in Latin script (§3.2).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnLocode(String);

impl UnLocode {
    pub fn parse(s: &str) -> CoreResult<UnLocode> {
        let raw = s.trim().to_ascii_uppercase().replace(' ', "");
        let bytes = raw.as_bytes();
        if bytes.len() != 5
            || !bytes[..2].iter().all(u8::is_ascii_uppercase)
            || !bytes[2..].iter().all(u8::is_ascii_uppercase)
        {
            return Err(CoreError::Validation(format!(
                "invalid UN/LOCODE {raw:?} (expected e.g. IRBND)"
            )));
        }
        Ok(UnLocode(raw))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UnLocode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for UnLocode {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        UnLocode::parse(s)
    }
}

/// A vessel registered in master data (bilingual name per PRD L-7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vessel {
    pub id: VesselId,
    pub imo: ImoNumber,
    pub name_en: String,
    pub name_fa: String,
    /// Maritime Mobile Service Identity, if known.
    pub mmsi: Option<String>,
    pub flag_code: Option<String>,
    /// Length overall in metres.
    pub loa_m: f64,
    /// Beam in metres.
    pub beam_m: f64,
    /// Maximum summer draft in metres.
    pub max_draft_m: f64,
    /// Deadweight tonnage.
    pub dwt_t: u32,
    /// Number of reefer sockets on board.
    pub reefer_points: u16,
    /// True when the vessel has no own gear (needs shore cranes).
    pub gearless: bool,
    /// Operating line / owner for billing.
    pub customer: Option<CustomerId>,
}

/// Operational phase of a vessel call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallStatus {
    Scheduled,
    AnchorageArrived,
    Berthed,
    Working,
    Completed,
    Departed,
    Cancelled,
}

/// Cargo profile announced for the call — drives berth scoring and
/// equipment planning (PRD F-2).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CargoMix {
    #[serde(default)]
    pub discharge_teu: u32,
    #[serde(default)]
    pub load_teu: u32,
    #[serde(default)]
    pub reefer_units: u16,
    #[serde(default)]
    pub dangerous_units: u16,
    #[serde(default)]
    pub over_gauge_units: u16,
}

impl CargoMix {
    /// Rough number of cranes needed for the working window.
    #[must_use]
    pub fn cranes_needed(&self) -> u8 {
        let work = self.discharge_teu + self.load_teu;
        // Ceiling: 900 TEU/crane per working window, minimum one crane.
        let base = ((work + 899) / 900).clamp(1, 8) as u8;
        if self.reefer_units > 0 || self.dangerous_units > 0 {
            base.saturating_add(1).min(8)
        } else {
            base
        }
    }
}

/// A vessel call (port visit) with its lifecycle timestamps (PRD F-1, F-3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VesselCall {
    pub id: CallId,
    pub vessel: VesselId,
    pub voyage_code: String,
    pub port: UnLocode,
    /// Expected arrival (predictable from AIS — F-3).
    pub eta_utc: DateTime<Utc>,
    /// Expected departure.
    pub etd_utc: Option<DateTime<Utc>>,
    pub status: CallStatus,
    pub cargo_mix: CargoMix,
    /// When the vessel physically anchored.
    pub anchorage_arrived_at: Option<DateTime<Utc>>,
    /// When all-fast alongside.
    pub berthed_at: Option<DateTime<Utc>>,
    /// When work finished alongside.
    pub work_completed_at: Option<DateTime<Utc>>,
    /// Actual departure.
    pub departed_at: Option<DateTime<Utc>>,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imo_known_valid() {
        // Ever Given (widely documented) — IMO 9811000 passes the scheme.
        assert!(ImoNumber::parse("9811000").is_ok());
        // Scheme example: 9074729.
        assert!(ImoNumber::parse("9074729").is_ok());
        assert!(ImoNumber::parse("9811001").is_err());
        assert!(ImoNumber::parse("981100").is_err());
        assert!(ImoNumber::parse("98110000").is_err());
        assert_eq!(imo_check_digit("981100").unwrap(), 0);
    }

    #[test]
    fn imo_from_prefix_round_trips() {
        let imo = ImoNumber::from_prefix("123456").unwrap();
        assert!(ImoNumber::parse(imo.as_str()).is_ok());
    }

    #[test]
    fn unlocode_parsing() {
        assert!(UnLocode::parse("irbnd").is_ok());
        assert_eq!(UnLocode::parse("irbnd").unwrap().as_str(), "IRBND");
        assert!(UnLocode::parse("IRBND1").is_err());
        assert!(UnLocode::parse("IR").is_err());
    }

    #[test]
    fn cranes_needed_bounds() {
        let mut mix = CargoMix { discharge_teu: 2500, load_teu: 500, ..Default::default() };
        assert_eq!(mix.cranes_needed(), 4);
        mix = CargoMix { discharge_teu: 100, ..Default::default() };
        assert_eq!(mix.cranes_needed(), 1);
        mix.dangerous_units = 10;
        assert_eq!(mix.cranes_needed(), 2);
    }
}

impl std::fmt::Display for CallStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = serde_json::to_string(self).unwrap_or_default();
        f.write_str(s.trim_matches('"'))
    }
}
