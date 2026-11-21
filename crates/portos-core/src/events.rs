//! Domain events (PRD F-26, I-1): a single typed event vocabulary shared by
//! SSE streams, webhooks and the outbox. Serialized payloads are stable
//! JSON (schema version in [`crate::EVENT_SCHEMA_VERSION`]).

use crate::container::ContainerNumber;
use crate::ids::{
    AppointmentId, CallId, CustomerId, GateTxId, IncidentId, InvoiceId, JobId, SlotId,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Concrete domain events. `tag = "type"` keeps payloads self-describing
/// for external consumers (open APIs — I-1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DomainEventKind {
    VesselCallCreated {
        call: CallId,
        vessel_imo: String,
        eta_utc: DateTime<Utc>,
    },
    CallStatusChanged {
        call: CallId,
        new_status: String,
    },
    BerthAllocated {
        call: CallId,
        berth_code: String,
        planned_from: DateTime<Utc>,
        planned_to: DateTime<Utc>,
        score: Option<f64>,
    },
    ContainerGateIn {
        gate_tx: GateTxId,
        container: Option<ContainerNumber>,
        direction: String,
    },
    ContainerGateOut {
        gate_tx: GateTxId,
        container: Option<ContainerNumber>,
    },
    ContainerPlacedInYard {
        container: ContainerNumber,
        slot: SlotId,
    },
    HoldPlaced {
        container: ContainerNumber,
        hold_kind: String,
        reason: String,
    },
    HoldReleased {
        container: ContainerNumber,
    },
    ReeferAlarm {
        container: ContainerNumber,
        temperature_c: f64,
        detail: String,
    },
    AppointmentBooked {
        appointment: AppointmentId,
        reference: String,
        slot_from: DateTime<Utc>,
    },
    JobCompleted {
        job: JobId,
        container: Option<ContainerNumber>,
    },
    InvoiceIssued {
        invoice: InvoiceId,
        number: String,
        customer: CustomerId,
        total_minor: i64,
        currency: String,
    },
    PaymentRecorded {
        number: String,
        amount_minor: i64,
    },
    EdiMessageReceived {
        message_type: String,
        containers: usize,
    },
    IncidentReported {
        incident: IncidentId,
        severity: String,
    },
}

/// Envelope wrapping every published event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainEvent {
    pub id: Uuid,
    pub at: DateTime<Utc>,
    /// Tenant / terminal code (multi-terminal tenancy — F-46).
    pub terminal: String,
    /// Event payload schema version for consumer compatibility.
    pub schema_version: String,
    #[serde(flatten)]
    pub kind: DomainEventKind,
}

impl DomainEvent {
    #[must_use]
    pub fn new(terminal: &str, kind: DomainEventKind) -> Self {
        Self {
            id: Uuid::now_v7(),
            at: Utc::now(),
            terminal: terminal.to_string(),
            schema_version: crate::EVENT_SCHEMA_VERSION.to_string(),
            kind,
        }
    }

    /// Stable event type name (also used for webhook subscription filters).
    #[must_use]
    pub fn event_type(&self) -> &'static str {
        match &self.kind {
            DomainEventKind::VesselCallCreated { .. } => "vessel_call.created",
            DomainEventKind::CallStatusChanged { .. } => "vessel_call.status_changed",
            DomainEventKind::BerthAllocated { .. } => "berth.allocated",
            DomainEventKind::ContainerGateIn { .. } => "container.gate_in",
            DomainEventKind::ContainerGateOut { .. } => "container.gate_out",
            DomainEventKind::ContainerPlacedInYard { .. } => "container.yard_placed",
            DomainEventKind::HoldPlaced { .. } => "container.hold_placed",
            DomainEventKind::HoldReleased { .. } => "container.hold_released",
            DomainEventKind::ReeferAlarm { .. } => "container.reefer_alarm",
            DomainEventKind::AppointmentBooked { .. } => "appointment.booked",
            DomainEventKind::JobCompleted { .. } => "job.completed",
            DomainEventKind::InvoiceIssued { .. } => "invoice.issued",
            DomainEventKind::PaymentRecorded { .. } => "invoice.payment_recorded",
            DomainEventKind::EdiMessageReceived { .. } => "edi.message_received",
            DomainEventKind::IncidentReported { .. } => "safety.incident_reported",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_with_type_and_round_trips() {
        let ev = DomainEvent::new(
            "IRBND-CT1",
            DomainEventKind::ContainerGateIn {
                gate_tx: GateTxId::generate(),
                container: Some(ContainerNumber::parse("MSKU3881107").unwrap()),
                direction: "export".into(),
            },
        );
        assert_eq!(ev.event_type(), "container.gate_in");
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"type\":\"container_gate_in\""));
        let back: DomainEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.event_type(), "container.gate_in");
    }
}
