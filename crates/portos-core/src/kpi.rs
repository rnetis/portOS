//! KPI computations (PRD F-31, F-32, §1.4 targets).
//!
//! Pure functions over timestamp slices so they can be unit-tested and
//! reused by dashboards, scheduled reports and the digital twin later.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A completed (or failed) move with gross timing.
#[derive(Debug, Clone, Copy)]
pub struct MoveSample {
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub success: bool,
}

/// A container that dwelled in the yard between gate-in and gate-out.
#[derive(Debug, Clone, Copy)]
pub struct DwellSample {
    pub gate_in: DateTime<Utc>,
    pub gate_out: DateTime<Utc>,
}

/// A truck that passed the gate.
#[derive(Debug, Clone, Copy)]
pub struct TruckSample {
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
}

/// A vessel that waited at anchorage before berthing.
#[derive(Debug, Clone, Copy)]
pub struct AnchorageSample {
    pub anchorage_arrived: DateTime<Utc>,
    pub berthed: DateTime<Utc>,
}

/// KPI block for one window.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperationsKpis {
    /// Gross moves per hour across working periods (PRD target +15%).
    pub gross_moves_per_hour: Option<f64>,
    /// Average yard dwell in hours (PRD target -20%).
    pub avg_dwell_hours: Option<f64>,
    /// Average gate-to-gate truck turnaround in minutes (PRD target -30%).
    pub avg_truck_turnaround_minutes: Option<f64>,
    /// Average anchorage wait in hours (PRD target -25%).
    pub avg_anchorage_wait_hours: Option<f64>,
    /// Yard occupancy 0..100.
    pub yard_occupancy_pct: Option<f64>,
    /// Gate transactions completed per hour in the window.
    pub gate_throughput_per_hour: Option<f64>,
    /// Open reefer alarms (PRD F-10).
    pub reefer_alarms_open: u32,
    /// Failed moves ratio 0..100.
    pub move_failure_pct: Option<f64>,
}

/// Gross crane moves per hour: total successful moves divided by the span
/// between the first move start and the last move end (gross working time).
#[must_use]
pub fn gross_moves_per_hour(moves: &[MoveSample]) -> Option<f64> {
    let ok: Vec<&MoveSample> = moves.iter().filter(|m| m.success).collect();
    let first = ok.iter().map(|m| m.started_at).min()?;
    let last = ok.iter().map(|m| m.finished_at).max()?;
    let hours = (last - first).num_seconds() as f64 / 3600.0;
    if hours <= 0.0 {
        return None;
    }
    Some(ok.len() as f64 / hours)
}

/// Mean dwell time in hours.
#[must_use]
pub fn avg_dwell_hours(samples: &[DwellSample]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let total: f64 = samples
        .iter()
        .map(|s| (s.gate_out - s.gate_in).num_seconds().max(0) as f64 / 3600.0)
        .sum();
    Some(total / samples.len() as f64)
}

/// Mean truck turnaround in minutes.
#[must_use]
pub fn avg_truck_turnaround_minutes(samples: &[TruckSample]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let total: f64 = samples
        .iter()
        .map(|s| (s.completed_at - s.started_at).num_seconds().max(0) as f64 / 60.0)
        .sum();
    Some(total / samples.len() as f64)
}

/// Mean anchorage wait in hours.
#[must_use]
pub fn avg_anchorage_wait_hours(samples: &[AnchorageSample]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let total: f64 = samples
        .iter()
        .map(|s| (s.berthed - s.anchorage_arrived).num_seconds().max(0) as f64 / 3600.0)
        .sum();
    Some(total / samples.len() as f64)
}

/// Failed moves as a percentage.
#[must_use]
pub fn move_failure_pct(moves: &[MoveSample]) -> Option<f64> {
    if moves.is_empty() {
        return None;
    }
    let failed = moves.iter().filter(|m| !m.success).count();
    Some((failed as f64) / (moves.len() as f64) * 100.0)
}

/// Transactions per hour over an explicit window.
#[must_use]
pub fn throughput_per_hour(count: u64, from: DateTime<Utc>, to: DateTime<Utc>) -> Option<f64> {
    let hours = (to - from).num_seconds() as f64 / 3600.0;
    if hours <= 0.0 {
        return None;
    }
    Some(count as f64 / hours)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn moves_per_hour() {
        let t0 = Utc::now();
        let moves: Vec<MoveSample> = (0..30)
            .map(|i| MoveSample {
                started_at: t0 + Duration::seconds(i * 120),
                finished_at: t0 + Duration::seconds(i * 120 + 100),
                success: true,
            })
            .collect();
        let mph = gross_moves_per_hour(&moves).unwrap();
        // 30 moves in ~0.99 hour (29×120s spans + 100s cycle).
        assert!(mph > 29.0 && mph < 31.0, "got {mph}");
    }

    #[test]
    fn failures_excluded_and_counted() {
        let t0 = Utc::now();
        let moves: Vec<MoveSample> = (0..10)
            .map(|i| MoveSample {
                started_at: t0 + Duration::minutes(i),
                finished_at: t0 + Duration::minutes(i + 1),
                success: i % 2 == 0,
            })
            .collect();
        assert!(gross_moves_per_hour(&moves).is_some());
        assert!((move_failure_pct(&moves).unwrap() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn dwell_and_truck_and_anchorage_means() {
        let t0 = Utc::now();
        let dwell = vec![
            DwellSample {
                gate_in: t0,
                gate_out: t0 + Duration::hours(24),
            },
            DwellSample {
                gate_in: t0,
                gate_out: t0 + Duration::hours(48),
            },
        ];
        assert!((avg_dwell_hours(&dwell).unwrap() - 36.0).abs() < 1e-9);

        let trucks = vec![TruckSample {
            started_at: t0,
            completed_at: t0 + Duration::seconds(300),
        }];
        assert!((avg_truck_turnaround_minutes(&trucks).unwrap() - 5.0).abs() < 1e-9);

        let anch = vec![AnchorageSample {
            anchorage_arrived: t0,
            berthed: t0 + Duration::hours(8),
        }];
        assert!((avg_anchorage_wait_hours(&anch).unwrap() - 8.0).abs() < 1e-9);
    }

    #[test]
    fn empty_windows_give_none() {
        assert!(gross_moves_per_hour(&[]).is_none());
        assert!(avg_dwell_hours(&[]).is_none());
        assert!(avg_truck_turnaround_minutes(&[]).is_none());
        assert!(avg_anchorage_wait_hours(&[]).is_none());
    }
}
