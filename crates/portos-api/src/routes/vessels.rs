//! Vessels, vessel calls, berths and berth allocation (PRD F-1..F-5).

use axum::extract::{Path, Query, State};
use axum::Extension;
use axum::Json;
use portos_core::berth::{self, Berth, BerthAllocation, CargoType, Occupancy, ScoringWeights};
use portos_core::ids::{AllocationId, BerthId, CallId, VesselId};
use portos_core::vessel::{
    CallStatus, CargoMix, ImoNumber, UnLocode, Vessel, VesselCall,
};
use portos_core::{CoreError, CoreResult};
use portos_i18n::Args;
use serde::Deserialize;
use std::collections::HashMap;

use crate::auth::AuthContext;
use crate::db::db;
use crate::error::{problem, ApiResult};
use crate::locale::LocaleCtx;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateVessel {
    pub imo: String,
    pub name_en: String,
    pub name_fa: String,
    pub mmsi: Option<String>,
    pub flag_code: Option<String>,
    pub loa_m: f64,
    pub beam_m: f64,
    pub max_draft_m: f64,
    pub dwt_t: u32,
    pub reefer_points: Option<u16>,
    pub gearless: Option<bool>,
}

/// POST /v1/vessels — register a vessel in master data (F-47).
pub async fn create_vessel(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<CreateVessel>,
) -> ApiResult<serde_json::Value> {
    auth.require("vessel.manage").map_err(|e| problem(&state, &ctx, e))?;
    let imo = ImoNumber::parse(&body.imo).map_err(|e| problem(&state, &ctx, e))?;
    let vessel = Vessel {
        id: VesselId::generate(),
        imo,
        name_en: body.name_en,
        name_fa: body.name_fa,
        mmsi: body.mmsi,
        flag_code: body.flag_code,
        loa_m: body.loa_m,
        beam_m: body.beam_m,
        max_draft_m: body.max_draft_m,
        dwt_t: body.dwt_t,
        reefer_points: body.reefer_points.unwrap_or(0),
        gearless: body.gearless.unwrap_or(true),
        customer: None,
    };
    let v = vessel.clone();
    db(&state.store, move |store| portos_store::fleet::create_vessel(store, &v))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(&state, &auth, "vessel.create", "vessel", &vessel.id.to_string()).await;
    let msg = state.t(
        ctx.locale,
        "vessel.created",
        &Args::new().arg("name", &vessel.name_en),
    );
    Ok(Json(serde_json::json!({ "id": vessel.id.to_string(), "imo": vessel.imo.as_str(), "message": msg })))
}

/// GET /v1/vessels
pub async fn list_vessels(
    State(state): State<AppState>,
    Extension(_ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
) -> ApiResult<serde_json::Value> {
    let vessels = db(&state.store, |store| portos_store::fleet::list_vessels(store))
        .await
        .map_err(|e| problem(&state, &_ctx, e))?;
    Ok(Json(serde_json::json!({ "vessels": vessels })))
}

/// GET /v1/vessels/{id}
pub async fn get_vessel(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let vid = parse_id::<VesselId>(&id).map_err(|e| problem(&state, &ctx, e))?;
    let vessel = db(&state.store, move |store| portos_store::fleet::get_vessel(store, vid))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    Ok(Json(serde_json::to_value(vessel).unwrap_or_default()))
}

fn parse_id<T: std::str::FromStr>(s: &str) -> CoreResult<T> {
    s.parse::<T>()
        .map_err(|_| CoreError::Validation(format!("invalid id {s:?}")))
}

#[derive(Deserialize)]
pub struct CreateCall {
    pub vessel_id: String,
    pub voyage_code: String,
    pub port: String,
    pub eta_utc: String,
    pub etd_utc: Option<String>,
    #[serde(default)]
    pub cargo_mix: CargoMix,
}

/// POST /v1/vessel-calls — vessel call registration with pre-arrival data
/// (F-1); publishes `vessel_call.created` (F-26).
pub async fn create_call(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<CreateCall>,
) -> ApiResult<serde_json::Value> {
    auth.require("vessel.manage").map_err(|e| problem(&state, &ctx, e))?;
    let vessel_id = parse_id::<VesselId>(&body.vessel_id).map_err(|e| problem(&state, &ctx, e))?;
    let port = UnLocode::parse(&body.port).map_err(|e| problem(&state, &ctx, e))?;
    let eta = chrono::DateTime::parse_from_rfc3339(&body.eta_utc)
        .map_err(|_| CoreError::Validation("eta_utc must be RFC 3339".into()))
        .map_err(|e| problem(&state, &ctx, e))?
        .with_timezone(&chrono::Utc);
    let etd = match &body.etd_utc {
        Some(s) => Some(
            chrono::DateTime::parse_from_rfc3339(s)
                .map_err(|_| CoreError::Validation("etd_utc must be RFC 3339".into()))
                .map_err(|e| problem(&state, &ctx, e))?
                .with_timezone(&chrono::Utc),
        ),
        None => None,
    };
    let call = VesselCall {
        id: CallId::generate(),
        vessel: vessel_id,
        voyage_code: body.voyage_code.clone(),
        port,
        eta_utc: eta,
        etd_utc: etd,
        status: CallStatus::Scheduled,
        cargo_mix: body.cargo_mix.clone(),
        anchorage_arrived_at: None,
        berthed_at: None,
        work_completed_at: None,
        departed_at: None,
        created_by: auth.username.clone(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    let c = call.clone();
    db(&state.store, move |store| portos_store::fleet::create_call(store, &c))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;

    // Vessel name for the event/notification.
    let vessel = db(&state.store, move |store| portos_store::fleet::get_vessel(store, vessel_id))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    crate::events::publish(
        &state,
        portos_core::events::DomainEventKind::VesselCallCreated {
            call: call.id,
            vessel_imo: vessel.imo.as_str().to_string(),
            eta_utc: call.eta_utc,
        },
    );
    crate::audit(&state, &auth, "vessel.call.create", "vessel_call", &call.id.to_string()).await;
    let eta_display = portos_i18n::formats::datetime_display(ctx.locale, ctx.calendar, ctx.numerals, call.eta_utc);
    let msg = state.t(
        ctx.locale,
        "vessel.call.created",
        &Args::new()
            .arg("vessel", &vessel.name_en)
            .arg("voyage", &call.voyage_code)
            .arg("eta", &eta_display),
    );
    Ok(Json(serde_json::json!({
        "id": call.id.to_string(),
        "status": call.status,
        "eta_display": eta_display,
        "message": msg,
    })))
}

/// GET /v1/vessel-calls?status=&from=&to=&limit=
pub async fn list_calls(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let status = q.get("status").and_then(|s| serde_json::from_str::<CallStatus>(&format!("\"{s}\"")).ok());
    let from = parse_ts_param(&q, "from");
    let to = parse_ts_param(&q, "to");
    let limit: u32 = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(50).min(500);
    let calls = db(&state.store, move |store| {
        portos_store::fleet::list_calls(store, status, from, to, limit)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    let calls = with_display_dates(&ctx, calls);
    Ok(Json(serde_json::json!({ "calls": calls })))
}

fn parse_ts_param(q: &HashMap<String, String>, key: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    q.get(key)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc))
}

/// Attach localized date displays so fa clients see Jalali dates (L-3)
/// while the API contract stays ISO 8601.
fn with_display_dates(ctx: &LocaleCtx, mut calls: Vec<VesselCall>) -> serde_json::Value {
    let arr: Vec<serde_json::Value> = calls
        .drain(..)
        .map(|c| {
            let mut v = serde_json::to_value(&c).unwrap_or_default();
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "eta_display".into(),
                    serde_json::Value::String(portos_i18n::formats::datetime_display(
                        ctx.locale, ctx.calendar, ctx.numerals, c.eta_utc,
                    )),
                );
            }
            v
        })
        .collect();
    serde_json::Value::Array(arr)
}

/// GET /v1/vessel-calls/{id}
pub async fn get_call(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let cid = parse_id::<CallId>(&id).map_err(|e| problem(&state, &ctx, e))?;
    let call = db(&state.store, move |store| portos_store::fleet::get_call(store, cid))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    let mut v = serde_json::to_value(&call).unwrap_or_default();
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "eta_display".into(),
            serde_json::Value::String(portos_i18n::formats::datetime_display(
                ctx.locale, ctx.calendar, ctx.numerals, call.eta_utc,
            )),
        );
    }
    Ok(Json(v))
}

#[derive(Deserialize)]
pub struct StatusChange {
    pub status: String,
    /// Optional timestamp for the transition (e.g. berthed_at).
    pub at: Option<String>,
}

/// POST /v1/vessel-calls/{id}/status — guarded lifecycle transitions
/// (F-1/F-5); stamps anchorage/berthed/departed times used by KPIs.
pub async fn change_status(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<StatusChange>,
) -> ApiResult<serde_json::Value> {
    auth.require("vessel.manage").map_err(|e| problem(&state, &ctx, e))?;
    let cid = parse_id::<CallId>(&id).map_err(|e| problem(&state, &ctx, e))?;
    let next: CallStatus = serde_json::from_str(&format!("\"{}\"", body.status))
        .map_err(|_| CoreError::Validation(format!("unknown status {}", body.status)))
        .map_err(|e| problem(&state, &ctx, e))?;
    let at = body
        .at
        .as_deref()
        .map(chrono::DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|_| CoreError::Validation("at must be RFC 3339".into()))
        .map_err(|e| problem(&state, &ctx, e))?
        .map(|d| d.with_timezone(&chrono::Utc));

    let current = db(&state.store, move |store| portos_store::fleet::get_call(store, cid))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    berth::transition_call(current.status, next).map_err(|e| problem(&state, &ctx, e))?;
    let cid2 = cid;
    db(&state.store, move |store| {
        portos_store::fleet::update_call_status(store, cid2, next, at)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::events::publish(
        &state,
        portos_core::events::DomainEventKind::CallStatusChanged {
            call: cid,
            new_status: next.to_string(),
        },
    );
    crate::audit(&state, &auth, "vessel.call.status", "vessel_call", &cid.to_string()).await;
    Ok(Json(serde_json::json!({ "id": cid.to_string(), "status": next })))
}

#[derive(Deserialize)]
pub struct EtaUpdate {
    pub eta_utc: String,
    pub etd_utc: Option<String>,
}

/// POST /v1/vessel-calls/{id}/eta — automatic re-planning input when the
/// predicted ETA shifts (F-3).
pub async fn update_eta(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<EtaUpdate>,
) -> ApiResult<serde_json::Value> {
    auth.require("vessel.manage").map_err(|e| problem(&state, &ctx, e))?;
    let cid = parse_id::<CallId>(&id).map_err(|e| problem(&state, &ctx, e))?;
    let eta = chrono::DateTime::parse_from_rfc3339(&body.eta_utc)
        .map_err(|_| CoreError::Validation("eta_utc must be RFC 3339".into()))
        .map_err(|e| problem(&state, &ctx, e))?
        .with_timezone(&chrono::Utc);
    let etd = body
        .etd_utc
        .as_deref()
        .map(chrono::DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|_| CoreError::Validation("etd_utc must be RFC 3339".into()))
        .map_err(|e| problem(&state, &ctx, e))?
        .map(|d| d.with_timezone(&chrono::Utc));
    db(&state.store, move |store| {
        portos_store::fleet::update_call_eta(store, cid, eta, etd)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(&state, &auth, "vessel.call.eta", "vessel_call", &cid.to_string()).await;
    Ok(Json(serde_json::json!({ "id": cid.to_string(), "eta_utc": eta.to_rfc3339() })))
}

#[derive(Deserialize)]
pub struct CreateBerth {
    pub code: String,
    pub name_en: String,
    pub name_fa: String,
    pub length_m: f64,
    pub max_draught_m: f64,
    pub crane_count: u8,
    pub shore_power: Option<bool>,
    #[serde(default)]
    pub capabilities: Vec<CargoType>,
}

/// POST /v1/berths
pub async fn create_berth(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<CreateBerth>,
) -> ApiResult<serde_json::Value> {
    auth.require("berth.manage").map_err(|e| problem(&state, &ctx, e))?;
    let berth = Berth {
        id: BerthId::generate(),
        code: body.code,
        name_en: body.name_en,
        name_fa: body.name_fa,
        length_m: body.length_m,
        max_draught_m: body.max_draught_m,
        crane_count: body.crane_count,
        shore_power: body.shore_power.unwrap_or(false),
        capabilities: if body.capabilities.is_empty() { vec![CargoType::Container] } else { body.capabilities },
        active: true,
    };
    let b = berth.clone();
    db(&state.store, move |store| portos_store::fleet::create_berth(store, &b))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(&state, &auth, "berth.create", "berth", &berth.code).await;
    Ok(Json(serde_json::to_value(berth).unwrap_or_default()))
}

/// GET /v1/berths
pub async fn list_berths(
    State(state): State<AppState>,
    Extension(_ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
) -> ApiResult<serde_json::Value> {
    let berths = db(&state.store, |store| portos_store::fleet::list_berths(store))
        .await
        .map_err(|e| problem(&state, &_ctx, e))?;
    Ok(Json(serde_json::json!({ "berths": berths })))
}

#[derive(Deserialize)]
pub struct BerthRequestBody {
    pub priority: Option<u8>,
    pub requested_from: Option<String>,
    pub requested_to: Option<String>,
    pub reason: Option<String>,
}

/// POST /v1/vessel-calls/{id}/berth-request (F-1).
pub async fn create_berth_request(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<BerthRequestBody>,
) -> ApiResult<serde_json::Value> {
    auth.require("vessel.manage").map_err(|e| problem(&state, &ctx, e))?;
    let cid = parse_id::<CallId>(&id).map_err(|e| problem(&state, &ctx, e))?;
    let call = db(&state.store, move |store| portos_store::fleet::get_call(store, cid))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    let from = body
        .requested_from
        .as_deref()
        .map(chrono::DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|_| CoreError::Validation("requested_from must be RFC 3339".into()))
        .map_err(|e| problem(&state, &ctx, e))?
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or(call.eta_utc);
    let to = body
        .requested_to
        .as_deref()
        .map(chrono::DateTime::parse_from_rfc3339)
        .transpose()
        .map_err(|_| CoreError::Validation("requested_to must be RFC 3339".into()))
        .map_err(|e| problem(&state, &ctx, e))?
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or_else(|| call.etd_utc.unwrap_or(from + chrono::Duration::hours(24)));
    let req_id = AllocationId::generate();
    let priority = body.priority.unwrap_or(3).clamp(1, 5);
    let reason = body.reason.clone();
    db(&state.store, move |store| {
        portos_store::fleet::create_berth_request(store, req_id, cid, from, to, priority, reason.as_deref())
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(&state, &auth, "berth.request", "berth_request", &req_id.to_string()).await;
    let msg = state.t(ctx.locale, "vessel.berth.requested", &Args::new().arg("call", &cid.to_string()));
    Ok(Json(serde_json::json!({ "request_id": req_id.to_string(), "from": from.to_rfc3339(), "to": to.to_rfc3339(), "priority": priority, "message": msg })))
}

/// POST /v1/vessel-calls/{id}/berth-suggest — run the explainable
/// allocation engine (F-2, §6.6). Read-only; nothing is persisted.
pub async fn suggest_berths(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let cid = parse_id::<CallId>(&id).map_err(|e| problem(&state, &ctx, e))?;
    let (call, vessel, berths, occupancy, priority) = load_allocation_inputs(&state, cid, &ctx).await?;
    let suggestions = berth::suggest_berths(
        &vessel,
        &call,
        priority,
        &berths,
        &occupancy,
        chrono::Utc::now(),
        &ScoringWeights::default(),
    );
    Ok(Json(serde_json::json!({ "call": cid.to_string(), "suggestions": suggestions })))
}

/// POST /v1/vessel-calls/{id}/berth-allocate — human approves a suggestion
/// (body: {"suggestion_index": 0} or {"berth_id": "..."}); persists the
/// allocation and publishes `berth.allocated`.
pub async fn allocate_berth(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<serde_json::Value> {
    auth.require("berth.allocate").map_err(|e| problem(&state, &ctx, e))?;
    let cid = parse_id::<CallId>(&id).map_err(|e| problem(&state, &ctx, e))?;
    let (call, vessel, berths, occupancy, priority) = load_allocation_inputs(&state, cid, &ctx).await?;
    let suggestions = berth::suggest_berths(
        &vessel,
        &call,
        priority,
        &berths,
        &occupancy,
        chrono::Utc::now(),
        &ScoringWeights::default(),
    );

    let chosen: &berth::BerthSuggestion = match body.get("suggestion_index").and_then(|v| v.as_u64()) {
        Some(idx) => suggestions
            .get(idx as usize)
            .ok_or_else(|| CoreError::Validation("suggestion_index out of range".into()))
            .map_err(|e| problem(&state, &ctx, e))?,
        None => {
            let berth_filter = body.get("berth_id").and_then(|v| v.as_str());
            suggestions
                .iter()
                .find(|s| s.score.is_some() && berth_filter.map(|b| b == &s.berth.to_string()).unwrap_or(true))
                .ok_or_else(|| CoreError::Validation("no feasible berth suggestion".into()))
                .map_err(|e| problem(&state, &ctx, e))?
        }
    };
    if chosen.score.is_none() {
        return Err(problem(&state, &ctx, CoreError::Validation(format!("berth {} is not feasible", chosen.berth_code))));
    }
    let allocation = BerthAllocation {
        id: AllocationId::generate(),
        call: cid,
        berth: chosen.berth,
        planned_from: chosen.planned_from.expect("feasible suggestion has a window"),
        planned_to: chosen.planned_to.expect("feasible suggestion has a window"),
        score: chosen.score,
        reasons: chosen.reasons.clone(),
        decided_by: auth.username.clone(),
        decided_at: chrono::Utc::now(),
    };
    let a = allocation.clone();
    db(&state.store, move |store| portos_store::fleet::create_allocation(store, &a))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    if let Ok(Some((req_id, _, _, _, _))) =
        db(&state.store, move |store| portos_store::fleet::open_request_for_call(store, cid)).await
    {
        let _ = db(&state.store, move |store| {
            portos_store::fleet::mark_request_allocated(store, req_id)
        })
        .await;
    }
    crate::events::publish(
        &state,
        portos_core::events::DomainEventKind::BerthAllocated {
            call: cid,
            berth_code: chosen.berth_code.clone(),
            planned_from: allocation.planned_from,
            planned_to: allocation.planned_to,
            score: allocation.score,
        },
    );
    crate::audit(&state, &auth, "berth.allocate", "berth_allocation", &allocation.id.to_string()).await;
    let score = allocation.score.unwrap_or(0.0);
    let msg = state.t(
        ctx.locale,
        "vessel.berth.allocated",
        &Args::new()
            .arg("berth", &chosen.berth_code)
            .arg("call", &cid.to_string())
            .arg("score", format!("{score:.2}")),
    );
    Ok(Json(serde_json::json!({
        "allocation": allocation,
        "message": msg,
    })))
}

/// Load everything the allocation engine needs in one shot.
async fn load_allocation_inputs(
    state: &AppState,
    cid: CallId,
    ctx: &LocaleCtx,
) -> CoreResult<(VesselCall, Vessel, Vec<Berth>, Vec<Occupancy>, u8)> {
    let call = db(&state.store, move |s| portos_store::fleet::get_call(s, cid)).await?;
    let vid = call.vessel;
    let vessel = db(&state.store, move |s| portos_store::fleet::get_vessel(s, vid)).await?;
    let berths = db(&state.store, |s| portos_store::fleet::list_berths(s)).await?;
    let now = chrono::Utc::now();
    let occupancy = db(&state.store, move |s| {
        portos_store::fleet::occupancy_between(s, now, now + chrono::Duration::days(14))
    })
    .await?
    .into_iter()
    .map(|(berth, from, to)| Occupancy { berth, from, to, call: None })
    .collect();
    let priority = db(&state.store, move |s| {
        Ok(portos_store::fleet::open_request_for_call(s, cid)?
            .map(|(_, _, _, p, _)| p)
            .unwrap_or(3))
    })
    .await?;
    Ok((call, vessel, berths, occupancy, priority))
}
