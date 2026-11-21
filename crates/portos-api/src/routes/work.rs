//! Equipment and job dispatch endpoints (PRD F-13..F-15).

use axum::extract::{Path, Query, State};
use axum::Extension;
use axum::Json;
use portos_core::container::ContainerNumber;
use portos_core::ids::EquipmentId;
use portos_core::work::{dispatch_next, transition_job, Equipment, EquipmentKind, EquipmentStatus, Job, JobKind, JobStatus, MoveRecord};
use portos_core::{CoreError, CoreResult};
use serde::Deserialize;
use std::collections::HashMap;

use crate::auth::AuthContext;
use crate::db::db;
use crate::error::{problem, ApiResult};
use crate::locale::LocaleCtx;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct EquipmentBody {
    pub code: String,
    pub kind: String,
    pub hours_meter: Option<f64>,
}

/// POST /v1/work/equipment — register equipment (cranes, SCs, AGVs — F-13).
pub async fn create_equipment(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<EquipmentBody>,
) -> ApiResult<serde_json::Value> {
    auth.require("work.manage").map_err(|e| problem(&state, &ctx, e))?;
    let kind: EquipmentKind = serde_json::from_str(&format!("\"{}\"", body.kind))
        .map_err(|_| CoreError::Validation(format!("unknown equipment kind {}", body.kind)))
        .map_err(|e| problem(&state, &ctx, e))?;
    let eq = Equipment {
        id: EquipmentId::generate(),
        code: body.code,
        kind,
        status: EquipmentStatus::Available,
        hours_meter: body.hours_meter.unwrap_or(0.0),
        last_service_at: None,
        telemetry: None,
    };
    let e = eq.clone();
    db(&state.store, move |store| portos_store::work::upsert_equipment(store, &e))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(&state, &auth, "work.equipment.create", "equipment", &eq.code).await;
    Ok(Json(serde_json::to_value(&eq).unwrap_or_default()))
}

/// GET /v1/work/equipment
pub async fn list_equipment(
    State(state): State<AppState>,
    Extension(_ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
) -> ApiResult<serde_json::Value> {
    let equipment = db(&state.store, |store| portos_store::work::list_equipment(store))
        .await
        .map_err(|e| problem(&state, &_ctx, e))?;
    Ok(Json(serde_json::json!({ "equipment": equipment })))
}

#[derive(Deserialize)]
pub struct EquipmentStatusBody {
    pub status: String,
}

/// POST /v1/work/equipment/{id}/status
pub async fn set_equipment_status(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<EquipmentStatusBody>,
) -> ApiResult<serde_json::Value> {
    auth.require("work.manage").map_err(|e| problem(&state, &ctx, e))?;
    let eid = id
        .parse::<uuid::Uuid>()
        .map(EquipmentId::from_uuid)
        .map_err(|_| CoreError::Validation(format!("invalid equipment id {id:?}")))
        .map_err(|e| problem(&state, &ctx, e))?;
    let status: EquipmentStatus = serde_json::from_str(&format!("\"{}\"", body.status))
        .map_err(|_| CoreError::Validation(format!("unknown status {}", body.status)))
        .map_err(|e| problem(&state, &ctx, e))?;
    db(&state.store, move |store| portos_store::work::update_equipment_status(store, eid, status))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(&state, &auth, "work.equipment.status", "equipment", &id).await;
    Ok(Json(serde_json::json!({ "id": id, "status": status })))
}

#[derive(Deserialize)]
pub struct CreateJob {
    pub kind: String,
    pub container: Option<String>,
    pub vessel_call_id: Option<String>,
    pub priority: Option<u8>,
}

/// POST /v1/work/jobs — queue a discharge/load/shift job (F-6, F-13).
pub async fn create_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<CreateJob>,
) -> ApiResult<serde_json::Value> {
    auth.require("work.dispatch").map_err(|e| problem(&state, &ctx, e))?;
    let kind: JobKind = serde_json::from_str(&format!("\"{}\"", body.kind))
        .map_err(|_| CoreError::Validation(format!("unknown job kind {}", body.kind)))
        .map_err(|e| problem(&state, &ctx, e))?;
    let container = match &body.container {
        Some(c) => Some(ContainerNumber::parse(c).map_err(|e| problem(&state, &ctx, e))?),
        None => None,
    };
    let call = body
        .vessel_call_id
        .map(|s| s.parse::<uuid::Uuid>().map(portos_core::ids::CallId::from_uuid))
        .transpose()
        .map_err(|_| CoreError::Validation("bad vessel_call_id".into()))
        .map_err(|e| problem(&state, &ctx, e))?;
    // Dangerous goods and reefer moves are prioritized (F-8/F-10).
    let container_profile = match &container {
        Some(cn) => {
            let lookup = cn.clone();
            Some(
                db(&state.store, move |store| portos_store::yard::get_container(store, &lookup))
                    .await
                    .map_err(|e| problem(&state, &ctx, e))?,
            )
        }
        None => None,
    };
    let priority = body.priority.unwrap_or_else(|| match &container_profile {
        Some(c) if c.imdg_class.is_some() => 1,
        Some(c) if c.kind().needs_plug() => 2,
        _ => 5,
    });
    let job = Job {
        id: portos_core::ids::JobId::generate(),
        kind,
        call,
        container,
        assigned_to: None,
        priority: priority.clamp(1, 5),
        status: JobStatus::Queued,
        created_at: chrono::Utc::now(),
        assigned_at: None,
        completed_at: None,
        issue: None,
    };
    let j = job.clone();
    db(&state.store, move |store| portos_store::work::create_job(store, &j))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(&state, &auth, "work.job.create", "job", &job.id.to_string()).await;
    Ok(Json(serde_json::to_value(&job).unwrap_or_default()))
}

/// GET /v1/work/jobs?status=queued,assigned
pub async fn list_jobs(
    State(state): State<AppState>,
    Extension(_ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let statuses: Vec<JobStatus> = q
        .get("status")
        .map(|s| {
            s.split(',')
                .filter_map(|x| serde_json::from_str(&format!("\"{}\"", x.trim())).ok())
                .collect()
        })
        .unwrap_or_default();
    let limit: u32 = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100).min(1000);
    let jobs = db(&state.store, move |store| {
        portos_store::work::list_jobs(store, &statuses, limit)
    })
    .await
    .map_err(|e| problem(&state, &_ctx, e))?;
    Ok(Json(serde_json::json!({ "jobs": jobs })))
}

/// POST /v1/work/jobs/{id}/dispatch — pick the highest-priority queued job
/// and an available compatible equipment unit (F-13 dispatch).
pub async fn dispatch_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    auth.require("work.dispatch").map_err(|e| problem(&state, &ctx, e))?;
    let jid = id
        .parse::<uuid::Uuid>()
        .map(portos_core::ids::JobId::from_uuid)
        .map_err(|_| CoreError::Validation(format!("invalid job id {id:?}")))
        .map_err(|e| problem(&state, &ctx, e))?;
    let (jobs, equipment) = db(&state.store, move |store| {
        let jobs = portos_store::work::list_jobs(store, &[JobStatus::Queued], 100)?;
        let equipment = portos_store::work::list_equipment(store)?;
        Ok((jobs, equipment))
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    let Some((target_job, equipment_id)) = dispatch_next(&jobs, &equipment) else {
        return Err(problem(&state, &ctx, CoreError::Conflict(
            "no available compatible equipment for the queued job".into(),
        )));
    };
    if target_job != jid {
        return Err(problem(&state, &ctx, CoreError::Conflict(format!(
            "dispatcher selected job {target_job} first (priority order)"
        ))));
    }
    let mut job = jobs.iter().find(|j| j.id == jid).cloned().ok_or_else(|| {
        CoreError::NotFound(format!("job {jid}"))
    })?;
    transition_job(job.status, JobStatus::InProgress).map_err(|e| problem(&state, &ctx, e))?;
    job.assigned_to = Some(equipment_id);
    job.assigned_at = Some(chrono::Utc::now());
    job.status = JobStatus::InProgress;
    let j = job.clone();
    db(&state.store, move |store| portos_store::work::update_job(store, &j))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    db(&state.store, move |store| {
        portos_store::work::update_equipment_status(store, equipment_id, EquipmentStatus::Busy)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(&state, &auth, "work.job.dispatch", "job", &jid.to_string()).await;
    Ok(Json(serde_json::json!({
        "job": job,
        "equipment_id": equipment_id.to_string(),
        // The operator's task queue entry (F-14 mobile / in-cab).
        "task": {
            "title": format!("{:?} {}", job.kind, job.container.as_ref().map(|c| c.as_str()).unwrap_or("")),
            "instructions": "Follow the in-cab task steps; confirm each completion.",
        }
    })))
}

#[derive(Deserialize)]
pub struct CompleteJob {
    pub success: bool,
    pub duration_seconds: Option<f64>,
    pub issue: Option<String>,
}

/// POST /v1/work/jobs/{id}/complete — operator confirmation (F-14); records
/// the move for productivity KPIs (F-32) and publishes `job.completed`.
pub async fn complete_job(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<CompleteJob>,
) -> ApiResult<serde_json::Value> {
    auth.require("work.dispatch").map_err(|e| problem(&state, &ctx, e))?;
    let jid = id
        .parse::<uuid::Uuid>()
        .map(portos_core::ids::JobId::from_uuid)
        .map_err(|_| CoreError::Validation(format!("invalid job id {id:?}")))
        .map_err(|e| problem(&state, &ctx, e))?;
    let mut job = db(&state.store, move |store| portos_store::work::get_job(store, jid))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    let next = if body.success { JobStatus::Done } else { JobStatus::Failed };
    transition_job(job.status, next).map_err(|e| problem(&state, &ctx, e))?;
    let equipment = job.assigned_to.ok_or_else(|| CoreError::Conflict("job not dispatched".into())).map_err(|e| problem(&state, &ctx, e))?;
    let container = job.container.clone().ok_or_else(|| {
        CoreError::Conflict("job has no container; dispatch a containerized move".into())
    }).map_err(|e| problem(&state, &ctx, e))?;
    job.status = next;
    job.completed_at = Some(chrono::Utc::now());
    job.issue = body.issue.clone();
    let j = job.clone();
    let mv = MoveRecord {
        id: portos_core::ids::MoveId::generate(),
        job: jid,
        equipment,
        container: container.clone(),
        kind: job.kind,
        started_at: job.assigned_at.unwrap_or_else(chrono::Utc::now),
        finished_at: job.completed_at.unwrap_or_else(chrono::Utc::now),
        success: body.success,
    };
    db(&state.store, move |store| {
        portos_store::work::update_job(store, &j)?;
        portos_store::work::record_move(store, &mv)?;
        portos_store::work::update_equipment_status(store, equipment, EquipmentStatus::Available)?;
        Ok(())
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::events::publish(
        &state,
        portos_core::events::DomainEventKind::JobCompleted {
            job: jid,
            container: Some(container),
        },
    );
    crate::audit(&state, &auth, "work.job.complete", "job", &jid.to_string()).await;
    Ok(Json(serde_json::json!({ "job": job, "success": body.success })))
}
