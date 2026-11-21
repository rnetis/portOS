//! Gate appointments, gate transactions and paperless documents
//! (PRD F-17..F-21).

use crate::container::ContainerNumber;
use crate::error::{CoreError, CoreResult};
use crate::ids::{AppointmentId, GateTxId};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Direction of a truck visit through the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateDirection {
    /// Import delivery: container leaves the terminal on this truck.
    Import,
    /// Export receiving: container enters the terminal for loading.
    Export,
    /// Empty equipment pickup.
    EmptyPickup,
    /// Empty equipment return.
    EmptyReturn,
}

impl GateDirection {
    /// True when the truck drives INTO the yard with cargo/equipment.
    #[must_use]
    pub fn is_gate_in(self) -> bool {
        matches!(self, GateDirection::Export | GateDirection::EmptyReturn)
    }

    /// Terminal workflow step that completes the transaction.
    #[must_use]
    pub fn terminal_step(self) -> GateStepKind {
        if self.is_gate_in() {
            GateStepKind::YardHandover
        } else {
            GateStepKind::RampOut
        }
    }
}

impl fmt::Display for GateDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GateDirection::Import => write!(f, "import"),
            GateDirection::Export => write!(f, "export"),
            GateDirection::EmptyPickup => write!(f, "empty-pickup"),
            GateDirection::EmptyReturn => write!(f, "empty-return"),
        }
    }
}

/// A booked truck appointment slot (PRD F-17).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TruckAppointment {
    pub id: AppointmentId,
    /// Human/business reference, e.g. `TA-2026-00042`.
    pub reference: String,
    pub trucking_company: Option<crate::ids::CustomerId>,
    pub driver_name: String,
    /// Driver license / national ID (verified at the gate — F-18).
    pub driver_license: String,
    /// Truck plate (OCR-checked at the gate).
    pub plate: String,
    pub container: Option<ContainerNumber>,
    pub direction: GateDirection,
    /// Slot window start ( congestion-based slot release adjusts these — F-17).
    pub slot_from: DateTime<Utc>,
    pub slot_to: DateTime<Utc>,
    pub status: AppointmentStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppointmentStatus {
    Booked,
    CheckedIn,
    Completed,
    NoShow,
    Cancelled,
}

/// One verification step inside a gate transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStepKind {
    /// Appointment / slot validation.
    AppointmentCheck,
    /// Driver identity verification (license/ID + biometrics at kiosk — F-18).
    DriverId,
    /// OCR container number + plate recognition.
    OcrScan,
    /// Seal integrity check (export/empties) or interchange condition.
    SealCheck,
    /// Customs/hold release check (F-11/F-24).
    ReleaseCheck,
    /// Yard handover — container transferred to terminal custody.
    YardHandover,
    /// Ramp out — container handed to the truck.
    RampOut,
}

/// Ordered workflow per direction.
#[must_use]
pub fn expected_steps(direction: GateDirection) -> Vec<GateStepKind> {
    vec![
        GateStepKind::AppointmentCheck,
        GateStepKind::DriverId,
        GateStepKind::OcrScan,
        GateStepKind::SealCheck,
        GateStepKind::ReleaseCheck,
        direction.terminal_step(),
    ]
}

/// A recorded step result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateStep {
    pub kind: GateStepKind,
    pub ok: bool,
    pub detail: Option<String>,
    pub at: DateTime<Utc>,
}

/// Status of a gate transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateStatus {
    InProgress,
    Completed,
    Rejected,
    /// Held for manual exception handling (F-21).
    Exception,
}

/// A gate transaction: the full, auditable passage of one truck.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GateTransaction {
    pub id: GateTxId,
    pub appointment: Option<AppointmentId>,
    pub direction: GateDirection,
    /// Expected container (from appointment / e-document).
    pub expected_container: Option<ContainerNumber>,
    /// Container as recognized by OCR; must equal expected at OcrScan.
    pub ocr_container: Option<ContainerNumber>,
    pub plate: String,
    pub driver_license: String,
    pub seal_number: Option<String>,
    pub gate_lane: String,
    pub steps: Vec<GateStep>,
    pub status: GateStatus,
    pub reject_reason: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

impl GateTransaction {
    /// Start a transaction for a truck at a lane. `expected_container` may
    /// be absent for ad-hoc visits (exception workflow — F-21).
    pub fn start(
        direction: GateDirection,
        appointment: Option<AppointmentId>,
        expected_container: Option<ContainerNumber>,
        plate: &str,
        driver_license: &str,
        gate_lane: &str,
        at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: GateTxId::generate(),
            appointment,
            direction,
            expected_container,
            ocr_container: None,
            plate: plate.to_string(),
            driver_license: driver_license.to_string(),
            seal_number: None,
            gate_lane: gate_lane.to_string(),
            steps: Vec::new(),
            status: GateStatus::InProgress,
            reject_reason: None,
            started_at: at,
            completed_at: None,
        }
    }

    /// Apply the next step in the workflow. Enforces step order, OCR match
    /// and terminal-step completion. Total elapsed time is measurable for
    /// the <5s gate transaction target (§6.1).
    pub fn apply(
        &mut self,
        kind: GateStepKind,
        ok: bool,
        detail: Option<String>,
        at: DateTime<Utc>,
    ) -> CoreResult<()> {
        if self.status != GateStatus::InProgress {
            return Err(CoreError::Conflict(format!(
                "gate transaction already {:?}",
                self.status
            )));
        }
        let expected = expected_steps(self.direction);
        let idx = self.steps.len();
        if idx >= expected.len() {
            return Err(CoreError::Conflict("no further steps expected".into()));
        }
        if expected[idx] != kind {
            return Err(CoreError::Validation(format!(
                "expected step {:?}, got {kind:?}",
                expected[idx]
            )));
        }
        // OCR must match the expected container when one is known.
        if kind == GateStepKind::OcrScan && ok {
            if let (Some(expected_no), Some(scanned)) =
                (&self.expected_container, &self.ocr_container)
            {
                if expected_no != scanned {
                    self.steps.push(GateStep {
                        kind,
                        ok: false,
                        detail: Some(format!(
                            "OCR mismatch: scanned {scanned}, expected {expected_no}"
                        )),
                        at,
                    });
                    self.status = GateStatus::Exception;
                    self.reject_reason = Some("ocr_mismatch".into());
                    return Ok(());
                }
            }
        }
        self.steps.push(GateStep {
            kind,
            ok,
            detail,
            at,
        });
        if !ok {
            self.status = GateStatus::Rejected;
            self.reject_reason = Some(kind.to_string());
            self.completed_at = Some(at);
            return Ok(());
        }
        if kind == self.direction.terminal_step() {
            self.status = GateStatus::Completed;
            self.completed_at = Some(at);
        }
        Ok(())
    }

    /// Attach an OCR result before applying the OcrScan step.
    pub fn set_ocr(&mut self, scanned: Option<ContainerNumber>) {
        self.ocr_container = scanned;
    }

    pub fn set_seal(&mut self, seal: &str) {
        self.seal_number = Some(seal.to_string());
    }

    /// Gate-to-gate elapsed time in seconds (KPI: truck turnaround §1.4).
    #[must_use]
    pub fn elapsed_seconds(&self) -> Option<f64> {
        self.completed_at
            .map(|c| (c - self.started_at).num_milliseconds() as f64 / 1000.0)
    }
}

impl fmt::Display for GateStepKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            GateStepKind::AppointmentCheck => "appointment-check",
            GateStepKind::DriverId => "driver-id",
            GateStepKind::OcrScan => "ocr-scan",
            GateStepKind::SealCheck => "seal-check",
            GateStepKind::ReleaseCheck => "release-check",
            GateStepKind::YardHandover => "yard-handover",
            GateStepKind::RampOut => "ramp-out",
        };
        f.write_str(s)
    }
}

/// Digital gate pass issued after a successful gate-in (PRD F-19).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatePass {
    /// Pass number, e.g. `GP-2026-001234`.
    pub pass_no: String,
    pub gate_tx: GateTxId,
    pub container: Option<ContainerNumber>,
    pub plate: String,
    pub direction: GateDirection,
    pub issued_at: DateTime<Utc>,
    /// QR-code payload for the kiosk/phone (F-19, F-21).
    pub qr_payload: String,
    pub valid_until: DateTime<Utc>,
}

impl GatePass {
    #[must_use]
    pub fn issue(tx: &GateTransaction, seq: u64) -> Self {
        let pass_no = format!("GP-{}-{seq:06}", tx.started_at.format("%Y"));
        Self {
            qr_payload: format!("PORTOS:PASS:{pass_no}:{}", tx.id),
            pass_no,
            gate_tx: tx.id,
            container: tx.expected_container.clone(),
            plate: tx.plate.clone(),
            direction: tx.direction,
            issued_at: tx.completed_at.unwrap_or_else(Utc::now),
            valid_until: tx.completed_at.unwrap_or_else(Utc::now) + Duration::hours(24),
        }
    }
}

/// Electronic Equipment Interchange Report — replaces the paper EIR (F-19).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquipmentInterchangeReport {
    /// EIR number, e.g. `EIR-2026-004567`.
    pub eir_no: String,
    pub gate_tx: GateTxId,
    pub container: ContainerNumber,
    /// Overall condition: `OK`, `DAMAGED`, `REPAIR`.
    pub condition: String,
    /// Free-text damage records.
    pub damages: Vec<String>,
    pub signed_by: String,
    pub at: DateTime<Utc>,
}

impl EquipmentInterchangeReport {
    #[must_use]
    pub fn create(tx: &GateTransaction, container: ContainerNumber, seq: u64) -> Self {
        Self {
            eir_no: format!(
                "EIR-{}-{seq:06}",
                tx.completed_at.unwrap_or_else(Utc::now).format("%Y")
            ),
            gate_tx: tx.id,
            container,
            condition: "OK".into(),
            damages: Vec::new(),
            signed_by: format!("driver:{}", tx.driver_license),
            at: tx.completed_at.unwrap_or_else(Utc::now),
        }
    }

    pub fn add_damage(&mut self, description: &str) {
        self.condition = "DAMAGED".into();
        self.damages.push(description.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn container() -> ContainerNumber {
        ContainerNumber::parse("MSKU3881107").unwrap()
    }

    #[test]
    fn happy_path_import_flow() {
        let t0 = Utc::now();
        let mut tx = GateTransaction::start(
            GateDirection::Import,
            None,
            Some(container()),
            "12A34567",
            "DL-123",
            "L2",
            t0,
        );
        for kind in expected_steps(tx.direction) {
            if kind == GateStepKind::OcrScan {
                tx.set_ocr(Some(container()));
            }
            tx.apply(kind, true, None, t0).unwrap();
        }
        assert_eq!(tx.status, GateStatus::Completed);
        assert!(tx.elapsed_seconds().unwrap() >= 0.0);
    }

    #[test]
    fn wrong_step_order_rejected() {
        let t0 = Utc::now();
        let mut tx = GateTransaction::start(
            GateDirection::Export,
            None,
            Some(container()),
            "12A34567",
            "DL-123",
            "L1",
            t0,
        );
        assert!(tx.apply(GateStepKind::DriverId, true, None, t0).is_err());
        tx.apply(GateStepKind::AppointmentCheck, true, None, t0)
            .unwrap();
        tx.apply(GateStepKind::DriverId, true, None, t0).unwrap();
        assert!(tx.apply(GateStepKind::SealCheck, true, None, t0).is_err());
    }

    #[test]
    fn ocr_mismatch_sends_to_exception() {
        let t0 = Utc::now();
        let mut tx = GateTransaction::start(
            GateDirection::Export,
            None,
            Some(container()),
            "12A34567",
            "DL-123",
            "L1",
            t0,
        );
        tx.apply(GateStepKind::AppointmentCheck, true, None, t0)
            .unwrap();
        tx.apply(GateStepKind::DriverId, true, None, t0).unwrap();
        tx.set_ocr(Some(ContainerNumber::parse("CSQU3054383").unwrap()));
        tx.apply(GateStepKind::OcrScan, true, None, t0).unwrap();
        assert_eq!(tx.status, GateStatus::Exception);
        assert_eq!(tx.reject_reason.as_deref(), Some("ocr_mismatch"));
    }

    #[test]
    fn failed_step_rejects_and_locks() {
        let t0 = Utc::now();
        let mut tx = GateTransaction::start(
            GateDirection::Import,
            None,
            None,
            "12A34567",
            "DL-123",
            "L1",
            t0,
        );
        tx.apply(
            GateStepKind::AppointmentCheck,
            false,
            Some("no appointment".into()),
            t0,
        )
        .unwrap();
        assert_eq!(tx.status, GateStatus::Rejected);
        assert!(tx.apply(GateStepKind::DriverId, true, None, t0).is_err());
    }

    #[test]
    fn gate_pass_and_eir_issued() {
        let t0 = Utc::now();
        let mut tx = GateTransaction::start(
            GateDirection::Export,
            None,
            Some(container()),
            "12A34567",
            "DL-123",
            "L1",
            t0,
        );
        for kind in expected_steps(GateDirection::Export) {
            if kind == GateStepKind::OcrScan {
                tx.set_ocr(Some(container()));
            }
            tx.apply(kind, true, None, t0).unwrap();
        }
        let pass = GatePass::issue(&tx, 1);
        assert!(pass.pass_no.starts_with("GP-"));
        assert!(pass.qr_payload.contains(&pass.pass_no));
        let mut eir = EquipmentInterchangeReport::create(&tx, container(), 7);
        assert!(eir.eir_no.starts_with("EIR-"));
        assert_eq!(eir.condition, "OK");
        eir.add_damage("left door rail bent");
        assert_eq!(eir.condition, "DAMAGED");
        assert_eq!(eir.damages.len(), 1);
    }
}
