//! Yard management endpoints (PRD F-7..F-12).

use axum::extract::{Path, Query, State};
use axum::Extension;
use axum::Json;
use portos_core::container::{
    Container, ContainerNumber, ContainerState, CustomsStatus, Hold, HoldKind, IsoSizeType,
};
use portos_core::ids::BlockId;
use portos_core::yard::{self, AssignmentPolicy, BlockType, NextLeg, YardBlock};
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
pub struct CreateBlock {
    pub code: String,
    pub name_en: String,
    pub name_fa: String,
    pub block_type: String,
    pub bays: u16,
    pub rows: u16,
    pub tiers: u16,
    pub reefer_plugs: Option<u16>,
    pub distance_to_quay_m: Option<u32>,
    pub distance_to_gate_m: Option<u32>,
    pub create_slots: Option<bool>,
}

/// POST /v1/yard/blocks — define a yard block, optionally materializing
/// all slots (F-7, F-9).
pub async fn create_block(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<CreateBlock>,
) -> ApiResult<serde_json::Value> {
    auth.require("yard.manage").map_err(|e| problem(&state, &ctx, e))?;
    let block_type: BlockType = serde_json::from_str(&format!("\"{}\"", body.block_type))
        .map_err(|_| CoreError::Validation(format!("unknown block type {}", body.block_type)))
        .map_err(|e| problem(&state, &ctx, e))?;
    let block = YardBlock {
        id: BlockId::generate(),
        code: body.code,
        name_en: body.name_en,
        name_fa: body.name_fa,
        block_type,
        bays: body.bays,
        rows: body.rows,
        tiers: body.tiers,
        reefer_plugs: body.reefer_plugs.unwrap_or(0),
        distance_to_quay_m: body.distance_to_quay_m.unwrap_or(500),
        distance_to_gate_m: body.distance_to_gate_m.unwrap_or(500),
    };
    let pre_create = body.create_slots.unwrap_or(true);
    let b = block.clone();
    let slots = db(&state.store, move |store| {
        portos_store::yard::create_block(store, &b, pre_create)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(&state, &auth, "yard.block.create", "yard_block", &block.code).await;
    Ok(Json(serde_json::json!({
        "block": block,
        "slots_created": slots,
    })))
}

/// GET /v1/yard/blocks
pub async fn list_blocks(
    State(state): State<AppState>,
    Extension(_ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
) -> ApiResult<serde_json::Value> {
    let blocks = db(&state.store, |store| portos_store::yard::list_blocks(store))
        .await
        .map_err(|e| problem(&state, &_ctx, e))?;
    Ok(Json(serde_json::json!({ "blocks": blocks })))
}

/// GET /v1/yard/blocks/{id}/snapshot — planning snapshot for dashboards.
pub async fn block_snapshot(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let bid = id
        .parse::<uuid::Uuid>()
        .map(BlockId::from_uuid)
        .map_err(|_| CoreError::Validation(format!("invalid block id {id:?}")))
        .map_err(|e| problem(&state, &ctx, e))?;
    let block = db(&state.store, move |store| portos_store::yard::get_block(store, bid))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    let snap = db(&state.store, move |store| {
        portos_store::yard::block_snapshot(store, &block)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    Ok(Json(serde_json::to_value(&snap).unwrap_or_default()))
}

#[derive(Deserialize)]
pub struct UpsertContainer {
    pub number: String,
    pub size_type: String,
    pub weight_kg: Option<u32>,
    pub imdg_class: Option<String>,
    pub reefer_setpoint_c: Option<f64>,
    pub over_gauge: Option<bool>,
    pub seal_number: Option<String>,
    pub pol: Option<String>,
    pub pod: Option<String>,
}

/// POST /v1/yard/containers — register/upsert container master data
/// (validates ISO 6346 check digit; §3.2 Latin identifiers).
pub async fn upsert_container(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<UpsertContainer>,
) -> ApiResult<serde_json::Value> {
    auth.require("yard.manage").map_err(|e| problem(&state, &ctx, e))?;
    let container = build_container(body).map_err(|e| problem(&state, &ctx, e))?;
    let c = container.clone();
    db(&state.store, move |store| portos_store::yard::upsert_container(store, &c))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(&state, &auth, "yard.container.upsert", "container", container.number.as_str()).await;
    Ok(Json(serde_json::to_value(&container).unwrap_or_default()))
}

fn build_container(body: UpsertContainer) -> CoreResult<Container> {
    Ok(Container {
        number: ContainerNumber::parse(&body.number)?,
        size_type: IsoSizeType::parse(&body.size_type)?,
        weight_kg: body.weight_kg.unwrap_or(0),
        imdg_class: body.imdg_class,
        reefer_setpoint_c: body.reefer_setpoint_c,
        over_gauge: body.over_gauge.unwrap_or(false),
        seal_number: body.seal_number,
        customer: None,
        pol: body.pol,
        pod: body.pod,
    })
}

/// GET /v1/yard/containers?state=in_yard&limit=
pub async fn list_containers(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let filter = q.get("state").and_then(|s| {
        serde_json::from_str::<ContainerState>(&format!("\"{s}\"")).ok()
    });
    let limit: u32 = q.get("limit").and_then(|v| v.parse().ok()).unwrap_or(100).min(1000);
    let containers = db(&state.store, move |store| {
        portos_store::yard::list_containers(store, filter, limit)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    Ok(Json(serde_json::json!({ "containers": containers })))
}

/// GET /v1/yard/containers/{number}
pub async fn get_container(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Path(number): Path<String>,
) -> ApiResult<serde_json::Value> {
    let cn = ContainerNumber::parse(&number).map_err(|e| problem(&state, &ctx, e))?;
    let cn_a = cn.clone();
    let container = db(&state.store, move |store| portos_store::yard::get_container(store, &cn_a))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    let slot = db(&state.store, move |store| {
        portos_store::yard::find_slot_of_container(store, &cn)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    Ok(Json(serde_json::json!({
        "container": container,
        "slot": slot,
    })))
}

#[derive(Deserialize)]
pub struct PlaceRequest {
    /// Where the container heads next — drives proximity scoring.
    pub next_leg: Option<String>,
    /// Explicit target berth/gate code (informational).
    pub target: Option<String>,
}

/// POST /v1/yard/containers/{number}/place — run the segregation-aware
/// slot assignment engine and occupy the suggested slot (F-7/F-8/F-9);
/// publishes `container.yard_placed`.
pub async fn place_container(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(number): Path<String>,
    Json(body): Json<PlaceRequest>,
) -> ApiResult<serde_json::Value> {
    auth.require("yard.manage").map_err(|e| problem(&state, &ctx, e))?;
    let cn = ContainerNumber::parse(&number).map_err(|e| problem(&state, &ctx, e))?;
    let cn_a = cn.clone();
    let container = db(&state.store, move |store| portos_store::yard::get_container(store, &cn_a))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;

    // Refuse to place containers with active holds (F-11).
    let cn_b = cn.clone();
    let holds = db(&state.store, move |store| portos_store::yard::active_holds(store, &cn_b))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    if !holds.is_empty() {
        return Err(problem(&state, &ctx, CoreError::Conflict(format!(
            "container {} has {} active hold(s)",
            cn,
            holds.len()
        ))));
    }

    let blocks = db(&state.store, |store| portos_store::yard::list_blocks(store))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    let mut snapshots = Vec::new();
    for b in &blocks {
        let block = b.clone();
        let snap = db(&state.store, move |store| portos_store::yard::block_snapshot(store, &block))
            .await
            .map_err(|e| problem(&state, &ctx, e))?;
        snapshots.push(snap);
    }
    let meta: HashMap<BlockId, &YardBlock> = blocks.iter().map(|b| (b.id, b)).collect();
    let snap_refs: Vec<&portos_core::yard::BlockSnapshot> = snapshots.iter().collect();
    let next_leg = match body.next_leg.as_deref() {
        Some("gate") => NextLeg::Gate,
        Some("vessel") => NextLeg::Vessel,
        Some("rail") => NextLeg::Rail,
        _ => NextLeg::Unknown,
    };
    let suggestion = yard::suggest_slot(
        &container,
        &snap_refs,
        &meta,
        next_leg,
        &AssignmentPolicy::default(),
    )
    .map_err(|e| problem(&state, &ctx, e))?;
    let slot = suggestion.slot.expect("Ok suggestion carries a slot");

    let username = auth.username.clone();
    let cn_in_tx = cn.clone();
    let placed = db(&state.store, move |store| {
        let slot_id = portos_store::yard::occupy_slot(
            store,
            slot.block,
            slot.bay,
            slot.row,
            slot.tier,
            &cn_in_tx,
        )?;
        portos_store::yard::record_yard_move(
            store,
            &cn_in_tx,
            None,
            Some(slot_id),
            "initial placement",
            &format!("user:{username}"),
            chrono::Utc::now(),
        )?;
        portos_store::yard::set_container_state(store, &cn_in_tx, ContainerState::InYard)?;
        Ok(slot_id)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;

    crate::events::publish(
        &state,
        portos_core::events::DomainEventKind::ContainerPlacedInYard {
            container: cn.clone(),
            slot: placed,
        },
    );
    let cn_display = cn.clone();
    crate::audit(&state, &auth, "yard.container.place", "container", cn_display.as_str()).await;
    let msg = state.t(
        ctx.locale,
        "yard.container.placed",
        &Args::new().arg("container", cn.as_str()),
    );
    Ok(Json(serde_json::json!({
        "container": cn.as_str(),
        "block": suggestion.block_code,
        "bay": slot.bay,
        "row": slot.row,
        "tier": slot.tier,
        "score": suggestion.score,
        "reasons": suggestion.reasons,
        "message": msg,
    })))
}

#[derive(Deserialize)]
pub struct HoldRequest {
    pub kind: String,
    pub reason: String,
}

/// POST /v1/yard/containers/{number}/hold — place a customs/security/line
/// hold (F-11); publishes `container.hold_placed`.
pub async fn place_hold(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(number): Path<String>,
    Json(body): Json<HoldRequest>,
) -> ApiResult<serde_json::Value> {
    auth.require("yard.manage").map_err(|e| problem(&state, &ctx, e))?;
    let cn = ContainerNumber::parse(&number).map_err(|e| problem(&state, &ctx, e))?;
    let kind: HoldKind = serde_json::from_str(&format!("\"{}\"", body.kind))
        .map_err(|_| CoreError::Validation(format!("unknown hold kind {}", body.kind)))
        .map_err(|e| problem(&state, &ctx, e))?;
    let hold = Hold {
        id: portos_core::ids::HoldId::generate(),
        container: cn.clone(),
        kind,
        reason: body.reason.clone(),
        placed_by: auth.username.clone(),
        placed_at: chrono::Utc::now(),
        released_at: None,
        released_by: None,
    };
    let h = hold.clone();
    let cn_hold = cn.clone();
    let _ = &cn_hold;
    db(&state.store, move |store| portos_store::yard::place_hold(store, &h))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    crate::events::publish(
        &state,
        portos_core::events::DomainEventKind::HoldPlaced {
            container: cn.clone(),
            hold_kind: format!("{kind:?}").to_lowercase(),
            reason: body.reason,
        },
    );
    crate::audit(&state, &auth, "yard.hold.place", "container", cn.as_str()).await;
    let msg = state.t(
        ctx.locale,
        "yard.hold.placed",
        &Args::new().arg("container", cn.as_str()).arg("reason", &hold.reason),
    );
    Ok(Json(serde_json::json!({ "hold": hold, "message": msg })))
}

/// POST /v1/yard/holds/{id}/release
pub async fn release_hold(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    auth.require("yard.manage").map_err(|e| problem(&state, &ctx, e))?;
    let hid = id
        .parse::<uuid::Uuid>()
        .map(portos_core::ids::HoldId::from_uuid)
        .map_err(|_| CoreError::Validation(format!("invalid hold id {id:?}")))
        .map_err(|e| problem(&state, &ctx, e))?;
    let username = auth.username.clone();
    let released = db(&state.store, move |store| {
        portos_store::yard::release_hold(store, hid, &username)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    if !released {
        return Err(problem(&state, &ctx, CoreError::NotFound(format!("open hold {id}"))));
    }
    crate::audit(&state, &auth, "yard.hold.release", "hold", &id).await;
    Ok(Json(serde_json::json!({ "released": true, "hold": id })))
}

#[derive(Deserialize)]
pub struct ReeferReading {
    pub temperature_c: f64,
    pub power_ok: bool,
}

/// POST /v1/yard/containers/{number}/reefer-reading — telemetry ingest
/// (I-4); auto-raises a `container.reefer_alarm` event when the reading
/// deviates from the setpoint or power is lost (F-10).
pub async fn reefer_reading(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(number): Path<String>,
    Json(body): Json<ReeferReading>,
) -> ApiResult<serde_json::Value> {
    auth.require("yard.manage").map_err(|e| problem(&state, &ctx, e))?;
    let cn = ContainerNumber::parse(&number).map_err(|e| problem(&state, &ctx, e))?;
    let cn_a = cn.clone();
    let container = db(&state.store, move |store| portos_store::yard::get_container(store, &cn_a))
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    let setpoint = container
        .reefer_setpoint_c
        .ok_or_else(|| CoreError::Validation("container is not a reefer".into()))
        .map_err(|e| problem(&state, &ctx, e))?;

    let deviation = (body.temperature_c - setpoint).abs();
    let alarm = if !body.power_ok {
        Some(format!("power lost (setpoint {setpoint:.1}°C)"))
    } else if deviation > 2.0 {
        Some(format!(
            "temperature {:.1}°C deviates {} from setpoint {:.1}°C",
            body.temperature_c, deviation, setpoint
        ))
    } else {
        None
    };
    let cn_b = cn.clone();
    let alarm_for_db = alarm.clone();
    db(&state.store, move |store| {
        portos_store::yard::add_reefer_reading(
            store,
            &cn_b,
            chrono::Utc::now(),
            body.temperature_c,
            body.power_ok,
            alarm_for_db.as_deref(),
        )
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    if let Some(detail) = &alarm {
        crate::events::publish(
            &state,
            portos_core::events::DomainEventKind::ReeferAlarm {
                container: cn.clone(),
                temperature_c: body.temperature_c,
                detail: detail.clone(),
            },
        );
        let cn_display = cn.clone();
        crate::audit(&state, &auth, "yard.reefer.alarm", "container", cn_display.as_str()).await;
        let msg = state.t(
            ctx.locale,
            "yard.reefer.alarm",
            &Args::new()
                .arg("container", cn_display.as_str())
                .arg("temp", format!("{:.1}", body.temperature_c))
                .arg("detail", detail),
        );
        return Ok(Json(serde_json::json!({
            "container": cn_display.as_str(),
            "alarm": detail,
            "message": msg,
        })));
    }
    Ok(Json(serde_json::json!({
        "container": cn.as_str(),
        "temperature_c": body.temperature_c,
        "within_tolerance": true,
    })))
}
