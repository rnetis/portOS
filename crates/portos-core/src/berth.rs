//! Berths and the rule-based, explainable berth allocation engine
//! (PRD F-2, §6.6 — recommendations come with reasons and a human override).

use crate::error::{CoreError, CoreResult};
use crate::ids::{AllocationId, BerthId, CallId};
use crate::vessel::{CargoMix, CallStatus, Vessel, VesselCall};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Cargo types a berth can handle; drives hard capability filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CargoType {
    Container,
    BreakBulk,
    Bulk,
    RoRo,
}

/// A berth along the quay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Berth {
    pub id: BerthId,
    /// Quay position code, e.g. `B3`.
    pub code: String,
    pub name_en: String,
    pub name_fa: String,
    /// Usable quay length in metres.
    pub length_m: f64,
    /// Maximum alongside draught in metres.
    pub max_draught_m: f64,
    /// Quay cranes available at this position.
    pub crane_count: u8,
    /// Shore power sockets for cold-ironing (PRD F-42).
    pub shore_power: bool,
    pub capabilities: Vec<CargoType>,
    pub active: bool,
}

impl Berth {
    #[must_use]
    pub fn can_handle(&self, t: CargoType) -> bool {
        self.capabilities.contains(&t)
    }
}

/// A berth request attached to a vessel call (PRD F-1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BerthRequest {
    pub id: AllocationId,
    pub call: CallId,
    /// Preferred window (may be adjusted by the planner).
    pub requested_from: DateTime<Utc>,
    pub requested_to: DateTime<Utc>,
    /// 1 = highest priority (military, reefer-heavy, line contract…).
    pub priority: u8,
    pub reason: Option<String>,
}

/// One existing allocation that blocks a berth for an interval.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Occupancy {
    pub berth: BerthId,
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
    pub call: Option<CallId>,
}

/// One ranked recommendation from the allocation engine (§6.6: explainable).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BerthSuggestion {
    pub berth: BerthId,
    pub berth_code: String,
    /// The earliest feasible window found for this berth.
    pub planned_from: Option<DateTime<Utc>>,
    pub planned_to: Option<DateTime<Utc>>,
    /// Overall score in `0.0..=1.0`; `None` when infeasible.
    pub score: Option<f64>,
    /// Human-readable reasons for the score / infeasibility.
    pub reasons: Vec<String>,
}

impl BerthSuggestion {
    /// The engine's top recommendation or an error for a fully infeasible call.
    pub fn best(self) -> CoreResult<BerthSuggestion> {
        if self.score.is_some() {
            Ok(self)
        } else {
            Err(CoreError::Validation(format!(
                "no feasible berth: {}",
                self.reasons.join("; ")
            )))
        }
    }
}

/// An approved allocation persisted after a human accepts a suggestion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BerthAllocation {
    pub id: AllocationId,
    pub call: CallId,
    pub berth: BerthId,
    pub planned_from: DateTime<Utc>,
    pub planned_to: DateTime<Utc>,
    pub score: Option<f64>,
    pub reasons: Vec<String>,
    /// Username of the human who approved/overrode (§6.6 human override).
    pub decided_by: String,
    pub decided_at: DateTime<Utc>,
}

/// Weights for the scoring model. Tunable per terminal via configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoringWeights {
    /// How well the vessel fits the berth (0.80 utilization is ideal).
    pub fit: f64,
    /// Crane availability vs. cranes needed.
    pub cranes: f64,
    /// Vessel-call priority (1 highest … 5 lowest).
    pub priority: f64,
    /// Earliest start bonus (less waiting at anchorage — KPI §1.4).
    pub earliness: f64,
    /// Shore power match when the call carries reefers.
    pub shore_power: f64,
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self { fit: 0.30, cranes: 0.25, priority: 0.15, earliness: 0.20, shore_power: 0.10 }
    }
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// Map a `CallStatus` to the cargo types the call implies.
fn implied_cargo(_mix: &CargoMix) -> CargoType {
    CargoType::Container // Phase 1 targets container terminals (§10 Q3)
}

/// Find the earliest free interval of `needed` duration on one berth,
/// starting no earlier than `earliest`, given existing occupancy.
fn earliest_free_window(
    occupancy: &[(DateTime<Utc>, DateTime<Utc>)],
    earliest: DateTime<Utc>,
    needed: Duration,
) -> (DateTime<Utc>, DateTime<Utc>) {
    let mut sorted: Vec<(DateTime<Utc>, DateTime<Utc>)> = occupancy.to_vec();
    sorted.sort_by_key(|(f, _)| *f);
    let mut cursor = earliest;
    for (from, to) in &sorted {
        if *to <= cursor {
            continue;
        }
        if *from >= cursor + needed {
            break;
        }
        cursor = *to;
    }
    (cursor, cursor + needed)
}

/// Score one berth for one call. Pure function — easy to test and explain.
#[allow(clippy::too_many_lines)]
fn score_berth(
    berth: &Berth,
    vessel: &Vessel,
    call: &VesselCall,
    priority: u8,
    weights: &ScoringWeights,
    earliest: DateTime<Utc>,
    occupied: &[(DateTime<Utc>, DateTime<Utc>)],
) -> BerthSuggestion {
    let mut reasons = Vec::new();

    if !berth.active {
        reasons.push(format!("berth {} is inactive", berth.code));
        return BerthSuggestion {
            berth: berth.id,
            berth_code: berth.code.clone(),
            planned_from: None,
            planned_to: None,
            score: None,
            reasons,
        };
    }

    // --- hard feasibility -------------------------------------------------
    let loa = vessel.loa_m;
    let safety_margin = loa * 0.05;
    if loa + safety_margin > berth.length_m {
        reasons.push(format!(
            "LOA {loa:.0}m + 5% margin exceeds berth {} length {:.0}m",
            berth.code, berth.length_m
        ));
        return BerthSuggestion {
            berth: berth.id,
            berth_code: berth.code.clone(),
            planned_from: None,
            planned_to: None,
            score: None,
            reasons,
        };
    }
    if vessel.max_draft_m > berth.max_draught_m {
        reasons.push(format!(
            "draft {:.1}m exceeds berth {} max draught {:.1}m",
            vessel.max_draft_m, berth.code, berth.max_draught_m
        ));
        return BerthSuggestion {
            berth: berth.id,
            berth_code: berth.code.clone(),
            planned_from: None,
            planned_to: None,
            score: None,
            reasons,
        };
    }
    let cargo = implied_cargo(&call.cargo_mix);
    if !berth.can_handle(cargo) {
        reasons.push(format!(
            "berth {} cannot handle {cargo:?} cargo",
            berth.code
        ));
        return BerthSuggestion {
            berth: berth.id,
            berth_code: berth.code.clone(),
            planned_from: None,
            planned_to: None,
            score: None,
            reasons,
        };
    }

    // --- window -----------------------------------------------------------
    let needed = call
        .etd_utc
        .map(|etd| {
            let start = call.eta_utc.max(earliest);
            (etd - start).max(Duration::hours(4))
        })
        .unwrap_or_else(|| Duration::hours(24));
    let mut occ: Vec<(DateTime<Utc>, DateTime<Utc>)> =
        occupied.iter().copied().collect();
    occ.sort_by_key(|(f, _)| *f);
    let (from, to) = earliest_free_window(&occ, earliest, needed);
    let wait_hours = (from - earliest).num_minutes() as f64 / 60.0;

    // --- scores -----------------------------------------------------------
    let fit = loa / berth.length_m;
    let fit_score = clamp01(1.0 - ((fit - 0.80).abs() / 0.5));
    let cranes_needed = f64::from(call.cargo_mix.cranes_needed());
    let crane_score = clamp01(f64::from(berth.crane_count) / cranes_needed);
    let priority_score = clamp01((6 - i32::from(priority.clamp(1, 5))) as f64 / 5.0);
    let earliness_score = 1.0 / (1.0 + wait_hours / 12.0);
    let reefer_benefit =
        if call.cargo_mix.reefer_units > 0 && berth.shore_power { 1.0 } else { 0.0 };

    let total = weights.fit * fit_score
        + weights.cranes * crane_score
        + weights.priority * priority_score
        + weights.earliness * earliness_score
        + weights.shore_power * reefer_benefit;

    reasons.push(format!(
        "LOA {loa:.0}m fits berth length {:.0}m (utilization {:.0}%)",
        berth.length_m,
        fit * 100.0
    ));
    reasons.push(format!(
        "draft {:.1}m within max draught {:.1}m",
        vessel.max_draft_m, berth.max_draught_m
    ));
    reasons.push(format!(
        "cranes: {} available / {} needed",
        berth.crane_count, cranes_needed as u8
    ));
    if reefer_benefit > 0.0 {
        reasons.push("shore power available for reefer load".to_string());
    }
    reasons.push(format!(
        "earliest free window {} → {} (wait {:.1}h)",
        from.to_rfc3339(),
        to.to_rfc3339(),
        wait_hours
    ));
    if priority != 3 {
        reasons.push(format!("call priority {priority}"));
    }

    BerthSuggestion {
        berth: berth.id,
        berth_code: berth.code.clone(),
        planned_from: Some(from),
        planned_to: Some(to),
        score: Some(clamp01(total)),
        reasons,
    }
}

/// Rank every berth for a call. Returns suggestions sorted best-first.
/// Infeasible berths are included (with reasons) so planners see why
/// they were excluded — explainability per PRD §6.6.
#[must_use]
pub fn suggest_berths(
    vessel: &Vessel,
    call: &VesselCall,
    berth_request_priority: u8,
    berths: &[Berth],
    occupancy: &[Occupancy],
    earliest: DateTime<Utc>,
    weights: &ScoringWeights,
) -> Vec<BerthSuggestion> {
    let mut out = Vec::with_capacity(berths.len());
    for berth in berths {
        let berth_occ: Vec<(DateTime<Utc>, DateTime<Utc>)> = occupancy
            .iter()
            .filter(|o| o.berth == berth.id)
            .map(|o| (o.from, o.to))
            .collect();
        out.push(score_berth(
            berth,
            vessel,
            call,
            berth_request_priority,
            weights,
            earliest,
            &berth_occ,
        ));
    }
    out.sort_by(|a, b| {
        b.score
            .unwrap_or(f64::NEG_INFINITY)
            .total_cmp(&a.score.unwrap_or(f64::NEG_INFINITY))
    });
    out
}

/// State transition guard for call lifecycle (PRD F-1/F-5).
pub fn transition_call(call_status: CallStatus, next: CallStatus) -> CoreResult<CallStatus> {
    use CallStatus::*;
    let allowed: &[(CallStatus, &[CallStatus])] = &[
        (Scheduled, &[AnchorageArrived, Cancelled, Berthed]),
        (AnchorageArrived, &[Berthed, Departed, Cancelled]),
        (Berthed, &[Working, Departed, Completed]),
        (Working, &[Completed, Departed]),
        (Completed, &[Departed]),
        (Departed, &[]),
        (Cancelled, &[]),
    ];
    let ok = allowed
        .iter()
        .any(|(s, nexts)| *s == call_status && nexts.contains(&next));
    if ok {
        Ok(next)
    } else {
        Err(CoreError::Conflict(format!(
            "illegal call transition {call_status:?} -> {next:?}"
        )))
    }
}

impl fmt::Display for CargoType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CargoType::Container => write!(f, "container"),
            CargoType::BreakBulk => write!(f, "break-bulk"),
            CargoType::Bulk => write!(f, "bulk"),
            CargoType::RoRo => write!(f, "Ro-Ro"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{BerthId, CallId, VesselId};

    fn berth(code: &str, len: f64, draught: f64, cranes: u8, shore: bool) -> Berth {
        Berth {
            id: BerthId::generate(),
            code: code.into(),
            name_en: code.into(),
            name_fa: code.into(),
            length_m: len,
            max_draught_m: draught,
            crane_count: cranes,
            shore_power: shore,
            capabilities: vec![CargoType::Container],
            active: true,
        }
    }

    fn vessel(loa: f64, draft: f64) -> Vessel {
        Vessel {
            id: VesselId::generate(),
            imo: crate::vessel::ImoNumber::parse("9811000").unwrap(),
            name_en: "MV Test".into(),
            name_fa: "کشتی آزمون".into(),
            mmsi: None,
            flag_code: None,
            loa_m: loa,
            beam_m: 32.0,
            max_draft_m: draft,
            dwt_t: 100_000,
            reefer_points: 500,
            gearless: true,
            customer: None,
        }
    }

    fn call(vessel: VesselId, eta: DateTime<Utc>) -> VesselCall {
        VesselCall {
            id: CallId::generate(),
            vessel,
            voyage_code: "V001".into(),
            port: crate::vessel::UnLocode::parse("IRBND").unwrap(),
            eta_utc: eta,
            etd_utc: Some(eta + Duration::hours(20)),
            status: CallStatus::Scheduled,
            cargo_mix: CargoMix { discharge_teu: 1800, load_teu: 400, ..Default::default() },
            anchorage_arrived_at: None,
            berthed_at: None,
            work_completed_at: None,
            departed_at: None,
            created_by: "tester".into(),
            created_at: eta - Duration::days(2),
            updated_at: eta - Duration::days(2),
        }
    }

    #[test]
    fn ranks_feasible_berths_and_explains() {
        let t0 = Utc::now();
        let v = vessel(294.0, 14.0);
        let c = call(v.id, t0 + Duration::hours(2));
        let b1 = berth("B1", 320.0, 15.0, 4, true);
        let b2 = berth("B2", 240.0, 15.0, 2, false); // too short
        let b3 = berth("B3", 360.0, 13.0, 4, false); // too shallow
        let out = suggest_berths(&v, &c, 3, &[b1, b2, b3], &[], t0, &ScoringWeights::default());
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].berth_code, "B1");
        assert!(out[0].score.unwrap() > 0.0);
        assert!(out[0].reasons.iter().any(|r| r.contains("LOA")));
        assert!(out[1].score.is_none() && out[1].reasons.iter().any(|r| r.contains("LOA")));
        assert!(out[2].score.is_none() && out[2].reasons.iter().any(|r| r.contains("draught")));
    }

    #[test]
    fn respects_existing_occupancy() {
        let t0 = Utc::now();
        let v = vessel(200.0, 10.0);
        let c = call(v.id, t0);
        let b = berth("B1", 300.0, 15.0, 4, false);
        let occ = vec![Occupancy {
            berth: b.id,
            from: t0,
            to: t0 + Duration::hours(6),
            call: None,
        }];
        let out = suggest_berths(&v, &c, 3, &[b], &occ, t0, &ScoringWeights::default());
        let s = &out[0];
        assert!(s.score.is_some());
        let from = s.planned_from.unwrap();
        assert!(from >= t0 + Duration::hours(6), "must start after occupancy ends");
        assert!(s.reasons.iter().any(|r| r.contains("wait 6.0h") || r.contains("wait 6")));
    }

    #[test]
    fn lifecycle_transitions_guarded() {
        assert!(transition_call(CallStatus::Scheduled, CallStatus::AnchorageArrived).is_ok());
        assert!(transition_call(CallStatus::AnchorageArrived, CallStatus::Berthed).is_ok());
        assert!(transition_call(CallStatus::Scheduled, CallStatus::Working).is_err());
        assert!(transition_call(CallStatus::Departed, CallStatus::Berthed).is_err());
    }
}
