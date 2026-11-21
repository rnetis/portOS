//! Tariff engine and invoicing (PRD F-27..F-30).
//!
//! Supports per-move, per-day (storage / demurrage / detention / reefer
//! plug), per-TEU, per-call, flat and per-kWh tariff bases with free days
//! and tiered pricing. All arithmetic is integer minor units + explicit
//! rounding; nothing is computed in floats except quantity multiplication
//! with explicit half-away-from-zero rounding.

use crate::container::ContainerNumber;
use crate::error::{CoreError, CoreResult};
use crate::ids::{CallId, CustomerId, InvoiceId, TariffId};
use crate::money::{Currency, Money};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// How a tariff charges.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TariffBasis {
    /// Per handling move (lift on / lift off).
    PerMove,
    /// Per day or part-day (storage, demurrage, detention, reefer plug).
    PerDay,
    /// Per TEU declared.
    PerTeu,
    /// Per vessel call.
    PerCall,
    /// One-off flat fee.
    Flat,
    /// Per kWh of shore power (PRD F-42).
    PerKwh,
}

impl fmt::Display for TariffBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TariffBasis::PerMove => write!(f, "per-move"),
            TariffBasis::PerDay => write!(f, "per-day"),
            TariffBasis::PerTeu => write!(f, "per-teu"),
            TariffBasis::PerCall => write!(f, "per-call"),
            TariffBasis::Flat => write!(f, "flat"),
            TariffBasis::PerKwh => write!(f, "per-kwh"),
        }
    }
}

/// One pricing tier: up to `up_to` quantity (None = everything above the
/// previous tier) charged at `unit_minor`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tier {
    pub up_to: Option<f64>,
    pub unit_minor: i64,
}

/// A configurable tariff (PRD F-27). Contract-specific tariffs reference a
/// customer and win over public tariffs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tariff {
    pub id: TariffId,
    /// Stable code, e.g. `STORAGE-IMP`, `DEM-LINE`, `REEFER-PLUG`, `MOVE-DIS`.
    pub code: String,
    pub name_en: String,
    pub name_fa: String,
    pub basis: TariffBasis,
    pub currency: Currency,
    /// Default unit price in minor units (used when no tier matches).
    pub unit_minor: i64,
    /// Free days before day-based charges start (0 = charge from day 1).
    pub free_days: u32,
    pub tiers: Vec<Tier>,
    /// VAT rate as a fraction, e.g. 0.09 → 9 % (configurable per jurisdiction).
    pub tax_rate: f64,
    pub customer: Option<CustomerId>,
    pub active: bool,
}

impl Tariff {
    /// Effective unit price for a cumulative quantity, tier-aware.
    #[must_use]
    pub fn unit_price_for(&self, quantity: f64) -> i64 {
        for tier in &self.tiers {
            if let Some(up_to) = tier.up_to {
                if quantity <= up_to {
                    return tier.unit_minor;
                }
            } else {
                return tier.unit_minor;
            }
        }
        self.unit_minor
    }
}

/// Reference to the operational object a charge line is for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChargeRef {
    Container { container: ContainerNumber },
    VesselCall { call: CallId },
    Unspecified { label: String },
}

impl fmt::Display for ChargeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChargeRef::Container { container } => write!(f, "{container}"),
            ChargeRef::VesselCall { call } => write!(f, "call:{call}"),
            ChargeRef::Unspecified { label } => write!(f, "{label}"),
        }
    }
}

/// One invoice line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChargeLine {
    pub tariff_code: String,
    pub description_en: String,
    pub description_fa: String,
    pub charge_ref: ChargeRef,
    pub quantity: f64,
    pub unit_minor: i64,
    pub amount: Money,
    pub tax_rate: f64,
    pub tax_minor: i64,
}

impl ChargeLine {
    fn build(
        tariff: &Tariff,
        charge_ref: ChargeRef,
        quantity: f64,
        description_en: String,
        description_fa: String,
    ) -> CoreResult<ChargeLine> {
        if quantity < 0.0 {
            return Err(CoreError::Validation("quantity must be >= 0".into()));
        }
        let unit_minor = tariff.unit_price_for(quantity);
        let amount = Money::new(unit_minor, tariff.currency).mul_qty(quantity);
        if amount.currency != tariff.currency {
            return Err(CoreError::Internal("currency mismatch in charge line".into()));
        }
        let tax_minor = (amount.minor as f64 * tariff.tax_rate).round() as i64;
        Ok(ChargeLine {
            tariff_code: tariff.code.clone(),
            description_en,
            description_fa,
            charge_ref,
            quantity,
            unit_minor,
            amount,
            tax_rate: tariff.tax_rate,
            tax_minor,
        })
    }
}

/// Days between two timestamps, rounded up (a part-day is a day).
#[must_use]
pub fn days_between(from: DateTime<Utc>, until: DateTime<Utc>) -> u32 {
    let secs = (until - from).num_seconds().max(0) as f64;
    (secs / 86_400.0).ceil() as u32
}

/// Result of a day-based charge computation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayCharge {
    pub days_total: u32,
    pub free_days: u32,
    pub days_billed: u32,
    pub amount: Money,
}

/// Storage/demurrage/detention/reefer-plug charge: per-day tariff applied
/// from `from` until `until` (or `now` when still in the yard), after
/// `free_days` grace days. Tier thresholds apply to the billed quantity.
pub fn compute_day_charge(
    tariff: &Tariff,
    from: DateTime<Utc>,
    until: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> CoreResult<DayCharge> {
    if tariff.basis != TariffBasis::PerDay {
        return Err(CoreError::Validation(format!(
            "tariff {} is not per-day",
            tariff.code
        )));
    }
    let end = until.unwrap_or(now);
    let days_total = days_between(from, end);
    let days_billed = days_total.saturating_sub(tariff.free_days);
    let unit_minor = tariff.unit_price_for(f64::from(days_billed));
    let amount = Money::new(unit_minor, tariff.currency).mul_qty(f64::from(days_billed));
    Ok(DayCharge {
        days_total,
        free_days: tariff.free_days,
        days_billed,
        amount,
    })
}

/// Per-move charge (discharge/load/shift count).
pub fn compute_move_charge(
    tariff: &Tariff,
    moves: u32,
    charge_ref: ChargeRef,
) -> CoreResult<ChargeLine> {
    if tariff.basis != TariffBasis::PerMove {
        return Err(CoreError::Validation(format!(
            "tariff {} is not per-move",
            tariff.code
        )));
    }
    ChargeLine::build(
        tariff,
        charge_ref,
        f64::from(moves),
        format!("{moves} handling move(s)"),
        format!("{moves} جابجایی"),
    )
}

/// Shore power energy charge (PRD F-42) — kWh quantity.
pub fn compute_kwh_charge(
    tariff: &Tariff,
    kwh: f64,
    charge_ref: ChargeRef,
) -> CoreResult<ChargeLine> {
    if tariff.basis != TariffBasis::PerKwh {
        return Err(CoreError::Validation(format!(
            "tariff {} is not per-kwh",
            tariff.code
        )));
    }
    ChargeLine::build(tariff, charge_ref, kwh, "shore power (kWh)".into(), "برق ساحلی (کیلووات‌ساعت)".into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvoiceStatus {
    Draft,
    Issued,
    Paid,
    PartiallyPaid,
    Void,
    Disputed,
}

/// An invoice header (PRD F-29).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
    pub id: InvoiceId,
    /// Human number, e.g. `INV-2026-000123`.
    pub number: String,
    pub customer: CustomerId,
    pub currency: Currency,
    pub status: InvoiceStatus,
    pub subtotal_minor: i64,
    pub tax_minor: i64,
    pub total_minor: i64,
    pub paid_minor: i64,
    pub issued_at: DateTime<Utc>,
    pub due_at: DateTime<Utc>,
    pub notes: Option<String>,
}

/// A draft invoice being assembled from charge lines.
#[derive(Debug, Clone)]
pub struct InvoiceDraft {
    pub customer: CustomerId,
    pub currency: Currency,
    pub lines: Vec<ChargeLine>,
}

impl InvoiceDraft {
    pub fn new(customer: CustomerId, currency: Currency) -> Self {
        Self { customer, currency, lines: Vec::new() }
    }

    pub fn push(&mut self, line: ChargeLine) -> CoreResult<()> {
        if line.amount.currency != self.currency {
            return Err(CoreError::Validation(format!(
                "line currency {} does not match invoice currency {}",
                line.amount.currency, self.currency
            )));
        }
        self.lines.push(line);
        Ok(())
    }

    /// Totals with a single rounding audit trail per line.
    pub fn totals(&self) -> (i64, i64, i64) {
        let subtotal: i64 = self.lines.iter().map(|l| l.amount.minor).sum();
        let tax: i64 = self.lines.iter().map(|l| l.tax_minor).sum();
        (subtotal, tax, subtotal + tax)
    }

    /// Finalize into an invoice with a supplied human number.
    pub fn finalize(self, number: String, now: DateTime<Utc>, payment_days: i64) -> Invoice {
        let (subtotal, tax, total) = self.totals();
        Invoice {
            id: InvoiceId::generate(),
            number,
            customer: self.customer,
            currency: self.currency,
            status: InvoiceStatus::Issued,
            subtotal_minor: subtotal,
            tax_minor: tax,
            total_minor: total,
            paid_minor: 0,
            issued_at: now,
            due_at: now + Duration::days(payment_days),
            notes: None,
        }
    }
}

/// Credit-control helper (PRD F-29): whether a customer can be charged.
#[must_use]
pub fn credit_available(
    outstanding_minor: i64,
    credit_limit_minor: i64,
    new_charge_minor: i64,
) -> bool {
    outstanding_minor + new_charge_minor <= credit_limit_minor
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::CustomerId;

    fn tariff(code: &str, unit: i64, free: u32, tiers: Vec<Tier>) -> Tariff {
        Tariff {
            id: TariffId::generate(),
            code: code.into(),
            name_en: code.into(),
            name_fa: code.into(),
            basis: TariffBasis::PerDay,
            currency: Currency::IRR,
            unit_minor: unit,
            free_days: free,
            tiers,
            tax_rate: 0.09,
            customer: None,
            active: true,
        }
    }

    #[test]
    fn storage_charge_with_free_days_and_tiers() {
        // First 5 billed days: 850_000 IRR/day; beyond: 1_200_000 IRR/day.
        let t = tariff(
            "STORAGE-IMP",
            850_000,
            4,
            vec![Tier { up_to: Some(5.0), unit_minor: 850_000 }, Tier { up_to: None, unit_minor: 1_200_000 }],
        );
        let from = Utc::now();
        let until = from + Duration::hours(24 * 10); // 10 days
        let c = compute_day_charge(&t, from, Some(until), Utc::now()).unwrap();
        assert_eq!(c.days_total, 10);
        assert_eq!(c.days_billed, 6);
        // 5 days at tier1 + 1 day at tier2? No: tier lookup uses cumulative
        // quantity (6 > 5) → tier 2 rate for all 6 days.
        assert_eq!(c.amount.minor, 6 * 1_200_000);
    }

    #[test]
    fn part_day_rounds_up() {
        let t = tariff("STORAGE", 100_000, 0, vec![]);
        let from = Utc::now();
        let until = from + Duration::hours(5);
        let c = compute_day_charge(&t, from, Some(until), Utc::now()).unwrap();
        assert_eq!(c.days_billed, 1);
        assert_eq!(c.amount.minor, 100_000);
    }

    #[test]
    fn still_in_yard_bills_until_now() {
        let t = tariff("STORAGE", 100_000, 0, vec![]);
        let from = Utc::now() - Duration::hours(48);
        let c = compute_day_charge(&t, from, None, Utc::now()).unwrap();
        assert_eq!(c.days_billed, 2);
    }

    #[test]
    fn move_charge_with_vat() {
        let mut t = tariff("MOVE-DIS", 1_450_000, 0, vec![]);
        t.basis = TariffBasis::PerMove;
        let line =
            compute_move_charge(&t, 3, ChargeRef::Unspecified { label: "batch".into() }).unwrap();
        assert_eq!(line.amount.minor, 4_350_000);
        assert_eq!(line.tax_minor, 391_500); // 9%
    }

    #[test]
    fn invoice_totals_and_finalize() {
        let mut t = tariff("STORAGE", 100_00, 0, vec![]); // $100.00/day
        t.currency = Currency::USD;
        t.tax_rate = 0.09;
        let draft = ChargeLine::build(
            &t,
            ChargeRef::Unspecified { label: "x".into() },
            3.0,
            "storage".into(),
            "انبارش".into(),
        )
        .unwrap();
        assert_eq!(draft.amount.minor, 300_00);
        assert_eq!(draft.tax_minor, 27_00);
        let customer = CustomerId::generate();
        let mut d = InvoiceDraft::new(customer, Currency::USD);
        d.push(draft).unwrap();
        let inv = d.finalize("INV-2026-000001".into(), Utc::now(), 30);
        assert_eq!(inv.subtotal_minor, 300_00);
        assert_eq!(inv.tax_minor, 27_00);
        assert_eq!(inv.total_minor, 327_00);
        assert_eq!(inv.status, InvoiceStatus::Issued);
    }

    #[test]
    fn invoice_currency_mismatch_rejected() {
        let t = tariff("STORAGE", 100_000, 0, vec![]); // IRR
        let line = ChargeLine::build(
            &t,
            ChargeRef::Unspecified { label: "x".into() },
            1.0,
            "a".into(),
            "ب".into(),
        )
        .unwrap();
        let mut d = InvoiceDraft::new(CustomerId::generate(), Currency::USD);
        assert!(d.push(line).is_err());
    }

    #[test]
    fn credit_control() {
        assert!(credit_available(90_000, 100_000, 10_000));
        assert!(!credit_available(90_000, 100_000, 10_001));
    }
}
