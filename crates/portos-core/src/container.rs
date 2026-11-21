//! Containers: ISO 6346 identification, size/type codes, statuses and holds
//! (PRD F-8..F-11, §3.2 — technical identifiers stay in Latin script).

use crate::error::{CoreError, CoreResult};
use crate::ids::{CustomerId, HoldId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Values for ISO 6346 letters: A=10, skipping multiples of 11 (11, 22, 33).
const fn iso6346_letter_value(c: u8) -> Option<u32> {
    if !c.is_ascii_uppercase() {
        return None;
    }
    let idx = (c - b'A') as u32;
    let mut n = 10 + idx;
    if n >= 11 {
        n += 1;
    }
    if n >= 22 {
        n += 1;
    }
    if n >= 33 {
        n += 1;
    }
    Some(n)
}

/// Compute the ISO 6346 check digit for a 10-character owner+category+serial
/// string (4 letters + 6 digits).
pub fn iso6346_check_digit(code10: &str) -> CoreResult<u8> {
    let bytes = code10.as_bytes();
    if bytes.len() != 10 {
        return Err(CoreError::Validation("container code must be 10 chars + check digit".into()));
    }
    let mut sum: u64 = 0;
    for (i, &c) in bytes.iter().enumerate() {
        let v = if c.is_ascii_digit() {
            u64::from(c - b'0')
        } else {
            u64::from(iso6346_letter_value(c).ok_or_else(|| {
                CoreError::Validation(format!("invalid character {c:?} in container code"))
            })?)
        };
        sum += v * (1u64 << i); // 2^i for i in 0..10
    }
    let check = (sum % 11) as u8;
    Ok(if check == 10 { 0 } else { check })
}

/// A validated ISO 6346 container number: 4 letters + 6 digits + check digit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContainerNumber(String);

impl ContainerNumber {
    /// Parse and validate (check digit included).
    pub fn parse(s: &str) -> CoreResult<ContainerNumber> {
        let raw = s.trim().to_ascii_uppercase().replace([' ', '-'], "");
        let bytes = raw.as_bytes();
        if bytes.len() != 11 {
            return Err(CoreError::Validation(format!(
                "container number must have 11 characters, got {}: {raw:?}",
                bytes.len()
            )));
        }
        for (i, &c) in bytes.iter().enumerate() {
            let ok = if i < 4 { c.is_ascii_uppercase() } else { c.is_ascii_digit() };
            if !ok {
                return Err(CoreError::Validation(format!(
                    "invalid character at position {}: {c:?}",
                    i + 1
                )));
            }
        }
        let category = &raw[3..4];
        if !matches!(category, "U" | "J" | "Z") {
            return Err(CoreError::Validation(format!(
                "invalid category letter {category:?} (expected U, J or Z)"
            )));
        }
        let expected = iso6346_check_digit(&raw[..10])?;
        let actual: u8 = raw[10..].parse().map_err(|_| {
            CoreError::Validation("check digit is not a number".into())
        })?;
        if expected != actual {
            return Err(CoreError::Validation(format!(
                "invalid ISO 6346 check digit: expected {expected}, got {actual} ({raw})"
            )));
        }
        Ok(ContainerNumber(raw))
    }

    /// Build a valid number from a 10-char prefix by computing the check digit.
    pub fn from_prefix(code10: &str) -> CoreResult<ContainerNumber> {
        let prefix = code10.trim().to_ascii_uppercase();
        let check = iso6346_check_digit(&prefix)?;
        ContainerNumber::parse(&format!("{prefix}{check}"))
    }

    #[must_use]
    pub fn owner_code(&self) -> &str {
        &self.0[..3]
    }

    /// Equipment category: `U` freight container, `J` detachable freight
    /// container-related equipment, `Z` tag/identifiable internal use.
    #[must_use]
    pub fn category(&self) -> &str {
        &self.0[3..4]
    }

    #[must_use]
    pub fn check_digit(&self) -> u8 {
        self.0[10..].parse().unwrap_or(0)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContainerNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for ContainerNumber {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for ContainerNumber {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ContainerNumber::parse(s)
    }
}

/// Nominal length class derived from the ISO size/type code first digit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LengthClass {
    Twenty,
    Forty,
    FortyFive,
    Other,
}

/// Cargo kind of the equipment, derived from the size/type code group letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContainerKind {
    General,
    Ventilated,
    Insulated,
    Reefer,
    OpenTop,
    Platform,
    Tank,
    Special,
}

impl ContainerKind {
    /// Whether the container needs an electrical reefer plug (PRD F-10).
    #[must_use]
    pub fn needs_plug(self) -> bool {
        matches!(self, ContainerKind::Reefer | ContainerKind::Insulated)
    }
}

/// ISO 6346 size/type code, e.g. `22G1`, `45R1`, `42P1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IsoSizeType(String);

impl IsoSizeType {
    pub fn parse(s: &str) -> CoreResult<IsoSizeType> {
        let raw = s.trim().to_ascii_uppercase();
        let bytes = raw.as_bytes();
        if bytes.len() != 4 {
            return Err(CoreError::Validation(format!(
                "ISO size/type code must have 4 characters: {raw:?}"
            )));
        }
        if !bytes[0].is_ascii_digit() || !bytes[1].is_ascii_digit() || !bytes[3].is_ascii_digit() {
            return Err(CoreError::Validation(format!(
                "invalid ISO size/type code {raw:?} (expected two digits, letter, digit)"
            )));
        }
        if !bytes[2].is_ascii_uppercase() {
            return Err(CoreError::Validation(format!(
                "invalid ISO size/type code {raw:?} (third char must be a letter)"
            )));
        }
        Ok(IsoSizeType(raw))
    }

    /// Loose parse used for gate OCR fallbacks: unknown codes are accepted
    /// but flagged as `Other`.
    pub fn parse_lenient(s: &str) -> Option<IsoSizeType> {
        IsoSizeType::parse(s).ok()
    }

    #[must_use]
    pub fn length(&self) -> LengthClass {
        match self.0.as_bytes()[0] {
            b'1' => LengthClass::Twenty,
            b'2' => LengthClass::Twenty,
            b'4' => match self.0.as_bytes()[1] {
                b'5' => LengthClass::FortyFive,
                _ => LengthClass::Forty,
            },
            _ => LengthClass::Other,
        }
    }

    #[must_use]
    pub fn kind(&self) -> ContainerKind {
        match self.0.as_bytes()[2] {
            b'G' | b'U' => ContainerKind::General,
            b'V' => ContainerKind::Ventilated,
            b'B' => ContainerKind::Insulated,
            b'R' => ContainerKind::Reefer,
            b'T' => ContainerKind::Tank,
            b'P' => ContainerKind::Platform,
            b'O' => ContainerKind::OpenTop,
            _ => ContainerKind::Special,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IsoSizeType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Physical/logistical state of a container in the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerState {
    /// Announced (COPARN) but not yet arrived.
    Advertised,
    InYard,
    OnVessel,
    GateOut,
    MovedByRail,
    MovedByBarge,
}

/// Customs status (PRD F-24).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomsStatus {
    NotDeclared,
    Declared,
    Cleared,
    InspectionRequired,
    InspectionInProgress,
}

/// Kind of hold placed on a container (PRD F-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldKind {
    Customs,
    Security,
    ShippingLine,
    FreightPayment,
    Damaged,
}

/// A hold blocks release/movement until released.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hold {
    pub id: HoldId,
    pub container: ContainerNumber,
    pub kind: HoldKind,
    pub reason: String,
    pub placed_by: String,
    pub placed_at: DateTime<Utc>,
    pub released_at: Option<DateTime<Utc>>,
    pub released_by: Option<String>,
}

impl Hold {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.released_at.is_none()
    }
}

/// A container as known to the terminal (master + operational state).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Container {
    pub number: ContainerNumber,
    pub size_type: IsoSizeType,
    pub weight_kg: u32,
    /// IMDG class when carrying dangerous goods, e.g. `"3"`, `"8"`, `"1.4"`.
    pub imdg_class: Option<String>,
    /// Setpoint for reefers in °C.
    pub reefer_setpoint_c: Option<f64>,
    pub over_gauge: bool,
    pub seal_number: Option<String>,
    pub customer: Option<CustomerId>,
    /// UN/LOCODE of port of loading / discharge.
    pub pol: Option<String>,
    pub pod: Option<String>,
}

impl Container {
    #[must_use]
    pub fn kind(&self) -> ContainerKind {
        self.size_type.kind()
    }

    /// True when the box needs special segregation in the yard (PRD F-8).
    #[must_use]
    pub fn needs_special_handling(&self) -> bool {
        self.kind().needs_plug() || self.imdg_class.is_some() || self.over_gauge
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso6346_known_valid_numbers() {
        // Widely documented examples with valid check digits.
        for ok in ["CSQU3054383", "MSKU3881107", "TEMU1234565"] {
            let n = ContainerNumber::parse(ok);
            assert!(n.is_ok(), "{ok} should be valid: {:?}", n.err());
        }
        // TEMU 123456 with computed check digit must equal 5.
        assert_eq!(iso6346_check_digit("TEMU123456").unwrap(), 5);
    }

    #[test]
    fn iso6346_rejects_bad_check_digit() {
        assert!(ContainerNumber::parse("CSQU3054384").is_err());
        assert!(ContainerNumber::parse("CSQU305438").is_err()); // 10 chars
        assert!(ContainerNumber::parse("CSQU30543831").is_err()); // 12 chars
        assert!(ContainerNumber::parse("csqu305438x").is_err()); // bad check char
        assert!(ContainerNumber::parse("CSQU3054383").is_ok());
        assert!(ContainerNumber::parse("1SQU3054383").is_err()); // digit in owner
        assert!(ContainerNumber::parse("CSQX3054383").is_err()); // bad category
    }

    #[test]
    fn from_prefix_generates_valid() {
        let n = ContainerNumber::from_prefix("HLXU123456").unwrap();
        assert_eq!(n.check_digit(), iso6346_check_digit("HLXU123456").unwrap());
        assert!(ContainerNumber::parse(n.as_str()).is_ok());
    }

    #[test]
    fn letter_values_skip_multiples_of_eleven() {
        assert_eq!(iso6346_letter_value(b'A'), Some(10));
        assert_eq!(iso6346_letter_value(b'B'), Some(12)); // 11 skipped
        assert_eq!(iso6346_letter_value(b'L'), Some(23)); // 22 skipped
        assert_eq!(iso6346_letter_value(b'M'), Some(24));
        assert_eq!(iso6346_letter_value(b'X'), Some(36)); // 33 skipped
        assert_eq!(iso6346_letter_value(b'Y'), Some(37));
        assert_eq!(iso6346_letter_value(b'Z'), Some(38));
        assert_eq!(iso6346_letter_value(b'0'), None);
    }

    #[test]
    fn size_type_parsing() {
        let st = IsoSizeType::parse("45R1").unwrap();
        assert_eq!(st.length(), LengthClass::FortyFive);
        assert_eq!(st.kind(), ContainerKind::Reefer);
        assert!(st.kind().needs_plug());
        let g = IsoSizeType::parse("22G1").unwrap();
        assert_eq!(g.length(), LengthClass::Twenty);
        assert!(!g.kind().needs_plug());
        assert!(IsoSizeType::parse("45RX").is_err());
        assert!(IsoSizeType::parse("45G11").is_err());
    }
}
