//! Money and multi-currency support (PRD L-9, F-27..F-30).
//!
//! Amounts are stored as integer minor units to avoid float drift.
//! Iranian Rial has zero decimals; Toman display (1 Toman = 10 Rial) is a
//! presentation concern handled by the localization layer / config.

use crate::error::{CoreError, CoreResult};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

/// Currencies supported out of the box (extensible via configuration).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Currency {
    IRR,
    USD,
    EUR,
    AED,
    TRY,
    CNY,
}

impl Currency {
    /// Number of minor units per major unit (ISO 4217 minor unit code).
    #[must_use]
    pub fn decimals(self) -> u32 {
        match self {
            Currency::IRR => 0, // Rial has no subunit in practice
            Currency::USD | Currency::EUR | Currency::AED | Currency::TRY | Currency::CNY => 2,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Currency::IRR => "IRR",
            Currency::USD => "USD",
            Currency::EUR => "EUR",
            Currency::AED => "AED",
            Currency::TRY => "TRY",
            Currency::CNY => "CNY",
        }
    }

    /// Parse a 3-letter ISO 4217 code.
    pub fn parse(code: &str) -> CoreResult<Currency> {
        match code.trim().to_ascii_uppercase().as_str() {
            "IRR" => Ok(Currency::IRR),
            "USD" => Ok(Currency::USD),
            "EUR" => Ok(Currency::EUR),
            "AED" => Ok(Currency::AED),
            "TRY" => Ok(Currency::TRY),
            "CNY" => Ok(Currency::CNY),
            other => Err(CoreError::Validation(format!("unknown currency: {other}"))),
        }
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for Currency {
    type Err = CoreError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Currency::parse(s)
    }
}

/// An amount in minor units of a currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    pub minor: i64,
    pub currency: Currency,
}

impl Money {
    #[must_use]
    pub const fn new(minor: i64, currency: Currency) -> Self {
        Self { minor, currency }
    }

    #[must_use]
    pub const fn zero(currency: Currency) -> Self {
        Self { minor: 0, currency }
    }

    /// Parse from a human decimal string, e.g. `"1,250.50"` or `"1250.50"`.
    pub fn parse_major(s: &str, currency: Currency) -> CoreResult<Money> {
        let cleaned: String = s
            .chars()
            .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == ',')
            .collect::<String>()
            .replace(',', "");
        let negative = cleaned.starts_with('-');
        let digits = cleaned.trim_start_matches('-');
        let dec = currency.decimals();
        let (int_part, frac_part) = match digits.split_once('.') {
            Some((i, f)) => (i, f),
            None => (digits, ""),
        };
        if int_part.is_empty() && frac_part.is_empty() {
            return Err(CoreError::Validation(format!("invalid money: {s:?}")));
        }
        let int_val: i64 = if int_part.is_empty() {
            0
        } else {
            int_part
                .parse()
                .map_err(|_| CoreError::Validation(format!("invalid money: {s:?}")))?
        };
        let scale = 10i64.pow(dec);
        let mut minor = int_val
            .checked_mul(scale)
            .ok_or_else(|| CoreError::Validation("money overflow".into()))?;
        if dec > 0 {
            let mut frac = 0i64;
            let mut frac_digits = 0;
            for c in frac_part.chars().take(dec as usize) {
                if let Some(d) = c.to_digit(10) {
                    frac = frac * 10 + d as i64;
                    frac_digits += 1;
                } else {
                    return Err(CoreError::Validation(format!("invalid money: {s:?}")));
                }
            }
            for _ in frac_digits..dec {
                frac *= 10;
            }
            minor += frac;
        }
        if negative {
            minor = -minor;
        }
        Ok(Money { minor, currency })
    }

    /// Checked addition; fails on currency mismatch or overflow.
    pub fn checked_add(self, other: Money) -> CoreResult<Money> {
        if self.currency != other.currency {
            return Err(CoreError::Validation(format!(
                "currency mismatch: {} vs {}",
                self.currency, other.currency
            )));
        }
        let minor = self
            .minor
            .checked_add(other.minor)
            .ok_or_else(|| CoreError::Validation("money overflow".into()))?;
        Ok(Money { minor, currency: self.currency })
    }

    /// Checked subtraction.
    pub fn checked_sub(self, other: Money) -> CoreResult<Money> {
        self.checked_add(Money { minor: -other.minor, currency: other.currency })
    }

    /// Multiply a unit amount by a quantity, rounding half away from zero.
    #[must_use]
    pub fn mul_qty(self, quantity: f64) -> Money {
        let rounded = (self.minor as f64 * quantity).round();
        Money { minor: rounded as i64, currency: self.currency }
    }

    /// Canonical string, e.g. `1250050 IRR`, `1250.50 USD`.
    #[must_use]
    pub fn to_canonical(self) -> String {
        let dec = self.currency.decimals() as u32;
        let scale = 10i64.pow(dec);
        let sign = if self.minor < 0 { "-" } else { "" };
        let abs = self.minor.unsigned_abs();
        match dec {
            0 => format!("{sign}{abs} {}", self.currency),
            _ => format!("{sign}{}.{:0width$} {}", abs / scale as u64, abs % scale as u64, self.currency, width = dec as usize),
        }
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_canonical())
    }
}

/// One stored FX observation (loaded from admin API or bank feed; I-3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FxEntry {
    /// Base currency code, e.g. `USD`.
    pub base: Currency,
    /// Quote currency code, e.g. `IRR`.
    pub quote: Currency,
    /// How many quote units one base unit buys, e.g. 100_000.0 IRR per USD.
    pub rate: f64,
    pub as_of: DateTime<Utc>,
    /// Source label (`manual`, provider name…).
    pub source: String,
}

/// In-memory FX table used by the billing engine.
#[derive(Debug, Clone, Default)]
pub struct FxTable {
    entries: HashMap<(Currency, Currency), FxEntry>,
}

impl FxTable {
    pub fn insert(&mut self, entry: FxEntry) {
        self.entries.insert((entry.base, entry.quote), entry);
    }

    #[must_use]
    pub fn get(&self, base: Currency, quote: Currency) -> Option<&FxEntry> {
        self.entries.get(&(base, quote))
    }

    /// Convert `amount` into `target` currency, cross through USD when no
    /// direct rate exists. Rate staleness is the caller's concern (the
    /// store keeps `as_of`).
    pub fn convert(&self, amount: Money, target: Currency) -> CoreResult<Money> {
        if amount.currency == target {
            return Ok(amount);
        }
        let rate: f64 = if let Some(e) = self.get(amount.currency, target) {
            e.rate
        } else if let Some(e) = self.get(target, amount.currency) {
            if e.rate == 0.0 {
                return Err(CoreError::Validation("zero FX rate".into()));
            }
            1.0 / e.rate
        } else if let (Some(a), Some(b)) = (
            self.get(amount.currency, Currency::USD),
            self.get(Currency::USD, target),
        ) {
            a.rate * b.rate
        } else {
            return Err(CoreError::Validation(format!(
                "no FX path {} -> {}",
                amount.currency, target
            )));
        };
        if rate <= 0.0 {
            return Err(CoreError::Validation("invalid FX rate".into()));
        }
        // Rates quote MAJOR units; scale by each currency's minor units.
        let major = amount.minor as f64 / 10f64.powi(amount.currency.decimals() as i32);
        let minor = (major * rate * 10f64.powi(target.decimals() as i32)).round() as i64;
        Ok(Money { minor, currency: target })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_major_amounts() {
        let m = Money::parse_major("1,250.50", Currency::USD).unwrap();
        assert_eq!(m, Money::new(125_050, Currency::USD));
        let m = Money::parse_major("850000", Currency::IRR).unwrap();
        assert_eq!(m, Money::new(850_000, Currency::IRR));
        assert!(Money::parse_major("abc", Currency::USD).is_err());
    }

    #[test]
    fn canonical_round_trip() {
        let m = Money::new(123_456_789, Currency::IRR);
        assert_eq!(m.to_canonical(), "123456789 IRR");
        let m = Money::new(-12_345, Currency::USD);
        assert_eq!(m.to_canonical(), "-123.45 USD");
    }

    #[test]
    fn currency_mismatch_rejected() {
        let a = Money::new(100, Currency::USD);
        let b = Money::new(100, Currency::IRR);
        assert!(a.checked_add(b).is_err());
    }

    #[test]
    fn fx_direct_and_cross() {
        let mut fx = FxTable::default();
        fx.insert(FxEntry {
            base: Currency::USD,
            quote: Currency::IRR,
            rate: 100_000.0,
            as_of: Utc::now(),
            source: "demo".into(),
        });
        fx.insert(FxEntry {
            base: Currency::EUR,
            quote: Currency::USD,
            rate: 1.1,
            as_of: Utc::now(),
            source: "demo".into(),
        });
        let usd = Money::new(100_00, Currency::USD); // 100 USD
        let irr = fx.convert(usd, Currency::IRR).unwrap();
        assert_eq!(irr.minor, 10_000_000);
        let eur = Money::new(10_000, Currency::EUR); // 100 EUR
        let usd2 = fx.convert(eur, Currency::USD).unwrap();
        assert_eq!(usd2.minor, 110_00); // 110 USD
        let eur2 = fx.convert(Money::new(10_000_000, Currency::IRR), Currency::EUR);
        // no IRR->EUR path: cross via USD missing (no IRR->USD) -> error
        assert!(eur2.is_err());
    }

    #[test]
    fn mul_qty_rounds_half_away() {
        let m = Money::new(1500, Currency::USD);
        assert_eq!(m.mul_qty(2.5).minor, 3750);
        assert_eq!(Money::new(1, Currency::USD).mul_qty(0.25).minor, 0);
    }
}
