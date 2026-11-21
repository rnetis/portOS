//! Equipment, jobs and move records (PRD F-13..F-15).

use crate::error::{CoreError, CoreResult};
use crate::ids::{CallId, EquipmentId, JobId, MoveId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Terminal equipment classes (PRD F-13: cranes, straddle carriers,
/// tractors, reach stackers; AGVs supported).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentKind {
    QuayCrane,
    YardCrane,
    StraddleCarrier,
    TerminalTractor,
    ReachStacker,
    Agv,
}

impl fmt::Display for EquipmentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EquipmentKind::QuayCrane => write!(f, "quay-crane"),
            EquipmentKind::YardCrane => write!(f, "yard-crane"),
            EquipmentKind::StraddleCarrier => write!(f, "straddle-carrier"),
            EquipmentKind::TerminalTractor => write!(f, "terminal-tractor"),
            EquipmentKind::ReachStacker => write!(f, "reach-stacker"),
            EquipmentKind::Agv => write!(f, "agv"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentStatus {
    Available,
    Busy,
    Maintenance,
    Offline,
}

/// A piece of equipment with telemetry snapshot (feeds F-16 predictive
/// maintenance in later phases).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Equipment {
    pub id: EquipmentId,
    /// Human code, e.g. `QC-03`, `SC-114`.
    pub code: String,
    pub kind: EquipmentKind,
    pub status: EquipmentStatus,
    /// Operating hours (for maintenance scheduling).
    pub hours_meter: f64,
    pub last_service_at: Option<DateTime<Utc>>,
    /// Latest raw telemetry (OPC-UA/MQTT bridge writes here — I-4).
    pub telemetry: Option<serde_json::Value>,
}

/// Job types dispatched to equipment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Discharge,
    Load,
    Shift,
}

impl JobKind {
    /// Equipment kinds that can execute this job.
    #[must_use]
    pub fn compatible_kinds(self) -> &'static [EquipmentKind] {
        match self {
            JobKind::Discharge | JobKind::Load => &[EquipmentKind::QuayCrane],
            JobKind::Shift => &[
                EquipmentKind::YardCrane,
                EquipmentKind::StraddleCarrier,
                EquipmentKind::ReachStacker,
                EquipmentKind::Agv,
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Assigned,
    InProgress,
    Done,
    Failed,
    Cancelled,
}

/// A unit of work created from stowage/discharge plans (F-6, F-13).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    pub call: Option<CallId>,
    pub container: Option<crate::container::ContainerNumber>,
    /// Quay crane / yard equipment chosen.
    pub assigned_to: Option<EquipmentId>,
    /// Lower runs first (dangerous goods and reefer moves are prioritized).
    pub priority: u8,
    pub status: JobStatus,
    pub created_at: DateTime<Utc>,
    pub assigned_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Free-text issue reported by the operator (F-14).
    pub issue: Option<String>,
}

/// A completed physical move with gross timing (drives KPI F-32:
/// moves per hour).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveRecord {
    pub id: MoveId,
    pub job: JobId,
    pub equipment: EquipmentId,
    pub container: crate::container::ContainerNumber,
    pub kind: JobKind,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub success: bool,
}

/// Pick the next queued job + an available compatible piece of equipment.
/// Simple deterministic priority-FIFO dispatch (autonomous/AGV support
/// comes from the same interface in Phase 3 — F-13).
pub fn dispatch_next(
    jobs: &[Job],
    equipment: &[Equipment],
) -> Option<(JobId, EquipmentId)> {
    let mut queued: Vec<&Job> = jobs
        .iter()
        .filter(|j| j.status == JobStatus::Queued)
        .collect();
    queued.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.created_at.cmp(&b.created_at))
    });
    let job = queued.first()?;
    let equip = equipment.iter().find(|e| {
        e.status == EquipmentStatus::Available
            && job.kind.compatible_kinds().contains(&e.kind)
    })?;
    Some((job.id, equip.id))
}

/// Validate a status transition on a job.
pub fn transition_job(current: JobStatus, next: JobStatus) -> CoreResult<JobStatus> {
    use JobStatus::*;
    let ok = matches!(
        (current, next),
        (Queued, Assigned)
            | (Assigned, InProgress)
            | (Assigned, Cancelled)
            | (InProgress, Done)
            | (InProgress, Failed)
            | (Failed, Queued)
            | (Queued, Cancelled)
    );
    if ok {
        Ok(next)
    } else {
        Err(CoreError::Conflict(format!(
            "illegal job transition {current:?} -> {next:?}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eq(id: &str, kind: EquipmentKind, status: EquipmentStatus) -> Equipment {
        Equipment {
            id: EquipmentId::generate(),
            code: id.into(),
            kind,
            status,
            hours_meter: 100.0,
            last_service_at: None,
            telemetry: None,
        }
    }

    fn job(kind: JobKind, priority: u8) -> Job {
        Job {
            id: JobId::generate(),
            kind,
            call: None,
            container: None,
            assigned_to: None,
            priority,
            status: JobStatus::Queued,
            created_at: Utc::now(),
            assigned_at: None,
            completed_at: None,
            issue: None,
        }
    }

    #[test]
    fn dispatches_priority_first_to_compatible_equipment() {
        let qc1 = eq("QC-01", EquipmentKind::QuayCrane, EquipmentStatus::Available);
        let sc1 = eq("SC-01", EquipmentKind::StraddleCarrier, EquipmentStatus::Available);
        let sc_id = sc1.id;
        let jobs = vec![job(JobKind::Discharge, 5), job(JobKind::Shift, 1)];
        let (jid, eid) = dispatch_next(&jobs, &[qc1, sc1]).unwrap();
        assert_eq!(jid, jobs[1].id, "priority 1 (shift) runs first");
        assert_eq!(eid, sc_id, "shift job goes to yard equipment");
    }

    #[test]
    fn no_dispatch_when_busy_or_incompatible() {
        let qc_busy = eq("QC-01", EquipmentKind::QuayCrane, EquipmentStatus::Busy);
        let sc1 = eq("SC-01", EquipmentKind::StraddleCarrier, EquipmentStatus::Available);
        let jobs = vec![job(JobKind::Discharge, 1)];
        assert!(dispatch_next(&jobs, &[qc_busy, sc1]).is_none());
    }

    #[test]
    fn job_transitions_guarded() {
        assert!(transition_job(JobStatus::Queued, JobStatus::Assigned).is_ok());
        assert!(transition_job(JobStatus::Queued, JobStatus::InProgress).is_err());
        assert!(transition_job(JobStatus::InProgress, JobStatus::Done).is_ok());
        assert!(transition_job(JobStatus::Done, JobStatus::Queued).is_err());
    }
}
