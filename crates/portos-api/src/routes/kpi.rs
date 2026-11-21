//! KPI dashboard endpoint (PRD F-31, F-32, §1.4 targets).

use axum::extract::{Query, State};
use axum::Extension;
use axum::Json;
use portos_core::kpi::{
    avg_anchorage_wait_hours, avg_dwell_hours, avg_truck_turnaround_minutes, gross_moves_per_hour,
    move_failure_pct, throughput_per_hour, AnchorageSample, DwellSample, MoveSample,
    OperationsKpis, TruckSample,
};
use std::collections::HashMap;

use crate::auth::AuthContext;
use crate::db::db;
use crate::error::{problem, ApiResult};
use crate::locale::LocaleCtx;
use crate::state::AppState;

/// GET /v1/kpi?hours=24 — operations dashboard KPIs with localized display
/// strings (Persian users get Jalali dates and Persian digits — L-3/L-4).
pub async fn dashboard(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let hours: i64 = q
        .get("hours")
        .and_then(|v| v.parse().ok())
        .unwrap_or(24)
        .clamp(1, 24 * 30);
    let from = chrono::Utc::now() - chrono::Duration::hours(hours);
    let to = chrono::Utc::now();

    let (moves, calls, gate_txs, yard_stats, reefer_alarms) = db(&state.store, move |store| {
        let moves = portos_store::work::moves_between(store, from, to)?;
        // Calls that berthed in the window (anchorage wait — §1.4).
        let calls = portos_store::fleet::list_calls(
            store,
            None,
            Some(from - chrono::Duration::days(30)),
            Some(to),
            500,
        )?;
        let gate_txs = portos_store::gate::list_recent_gate_tx(store, from, to, 5000)?;
        let blocks = portos_store::yard::list_blocks(store)?;
        let mut occupied = 0u32;
        let mut capacity = 0u32;
        for b in &blocks {
            let snap = portos_store::yard::block_snapshot(store, b)?;
            occupied += snap.occupied;
            capacity += snap.capacity;
        }
        let alarms = portos_store::yard::reefer_open_alarms(store, from)?;
        Ok((
            moves,
            calls,
            gate_txs,
            (occupied, capacity),
            alarms.len() as u32,
        ))
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;

    let move_samples: Vec<MoveSample> = moves
        .iter()
        .map(|m| MoveSample {
            started_at: m.started_at,
            finished_at: m.finished_at,
            success: m.success,
        })
        .collect();
    let dwell_samples: Vec<DwellSample> = gate_txs
        .iter()
        .filter_map(|t| match (&t.expected_container, t.completed_at) {
            (Some(_), Some(completed)) => Some(DwellSample {
                gate_in: t.started_at.min(completed),
                gate_out: t.started_at.max(completed),
            }),
            _ => None,
        })
        .collect();
    let truck_samples: Vec<TruckSample> = gate_txs
        .iter()
        .filter_map(|t| {
            t.completed_at.map(|c| TruckSample {
                started_at: t.started_at,
                completed_at: c,
            })
        })
        .collect();
    let anchorage_samples: Vec<AnchorageSample> = calls
        .iter()
        .filter_map(|c| match (c.anchorage_arrived_at, c.berthed_at) {
            (Some(a), Some(b)) if b >= from && b <= to => Some(AnchorageSample {
                anchorage_arrived: a,
                berthed: b,
            }),
            _ => None,
        })
        .collect();

    let kpis = OperationsKpis {
        gross_moves_per_hour: gross_moves_per_hour(&move_samples),
        avg_dwell_hours: avg_dwell_hours(&dwell_samples),
        avg_truck_turnaround_minutes: avg_truck_turnaround_minutes(&truck_samples),
        avg_anchorage_wait_hours: avg_anchorage_wait_hours(&anchorage_samples),
        yard_occupancy_pct: if yard_stats.1 > 0 {
            Some(yard_stats.0 as f64 / yard_stats.1 as f64 * 100.0)
        } else {
            None
        },
        gate_throughput_per_hour: throughput_per_hour(
            gate_txs
                .iter()
                .filter(|t| t.status == portos_core::gate::GateStatus::Completed)
                .count() as u64,
            from,
            to,
        ),
        reefer_alarms_open: reefer_alarms,
        move_failure_pct: move_failure_pct(&move_samples),
    };

    let num = ctx.numerals;
    let f = |v: Option<f64>, d: u32| v.map(|x| portos_i18n::formats::number_display(num, x, d));
    Ok(Json(serde_json::json!({
        "window_hours": hours,
        "from": from.to_rfc3339(),
        "to": to.to_rfc3339(),
        "kpis": kpis,
        "display": {
            "locale": ctx.locale.tag(),
            "calendar": ctx.calendar,
            "moves_per_hour": f(kpis.gross_moves_per_hour, 1),
            "dwell_hours": f(kpis.avg_dwell_hours, 1),
            "truck_turnaround_minutes": f(kpis.avg_truck_turnaround_minutes, 1),
            "anchorage_wait_hours": f(kpis.avg_anchorage_wait_hours, 1),
            "yard_occupancy_pct": f(kpis.yard_occupancy_pct, 1),
            "gate_throughput_per_hour": f(kpis.gate_throughput_per_hour, 1),
        },
        // Live operational status for the dashboard (F-31).
        "live": {
            "moves_in_window": moves.len(),
            "gate_transactions_in_window": gate_txs.len(),
            "yard_occupied": yard_stats.0,
            "yard_capacity": yard_stats.1,
        }
    })))
}
