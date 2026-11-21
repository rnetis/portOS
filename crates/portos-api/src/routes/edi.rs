//! EDI import/export endpoints (PRD I-2).

use axum::extract::State;
use axum::Extension;
use axum::Json;
use portos_edi::{baplie::BaplieMessage, coarri::CoarriMessage, codeco::CodecoMessage, coparn::CoparnMessage, syntax::Interchange};
use portos_core::container::{Container, ContainerNumber, ContainerState, CustomsStatus};
use portos_core::work::JobKind;
use portos_core::{CoreError, CoreResult};
use portos_i18n::Args;
use serde::Deserialize;

use crate::auth::AuthContext;
use crate::db::db;
use crate::error::{problem, ApiResult};
use crate::locale::LocaleCtx;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct ImportBody {
    pub message_type: String,
    /// Raw EDIFACT text (with optional UNA).
    pub raw: String,
}

/// POST /v1/edi/import — parse COPARN / CODECO / COARRI / BAPLIE messages
/// and apply them to the operational data (I-2, F-6).
pub async fn import_message(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<ImportBody>,
) -> ApiResult<serde_json::Value> {
    auth.require("edi.import").map_err(|e| problem(&state, &ctx, e))?;
    let interchange = Interchange::parse(&body.raw)
        .map_err(|e| problem(&state, &ctx, CoreError::Validation(format!("EDI: {e}"))))?;

    let message_type = body.message_type.trim().to_ascii_uppercase();
    match message_type.as_str() {
        "COPARN" => {
            let msg = CoparnMessage::parse(&interchange)
                .map_err(|e| problem(&state, &ctx, CoreError::Validation(format!("COPARN: {e}"))))?;
            let containers = msg.entries.clone();
            let count = containers.len();
            db(&state.store, move |store| {
                for e in containers {
                    let ct = Container {
                        number: e.container,
                        size_type: e.size_type,
                        weight_kg: e.weight_kg.unwrap_or(0),
                        imdg_class: None,
                        reefer_setpoint_c: None,
                        over_gauge: false,
                        seal_number: None,
                        customer: None,
                        pol: msg.port.clone(),
                        pod: None,
                    };
                    portos_store::yard::upsert_container(store, &ct)?;
                    let target = match e.movement {
                        portos_edi::coparn::Movement::Load => ContainerState::Advertised,
                        portos_edi::coparn::Movement::Discharge | portos_edi::coparn::Movement::Shift => {
                            ContainerState::Advertised
                        }
                    };
                    portos_store::yard::set_container_state(store, &ct.number, target)?;
                }
                Ok(())
            })
            .await
            .map_err(|e| problem(&state, &ctx, e))?;
            respond_edi(&state, ctx, auth, "COPARN", count, msg.vessel_name).await
        }
        "CODECO" => {
            let msg = CodecoMessage::parse(&interchange)
                .map_err(|e| problem(&state, &ctx, CoreError::Validation(format!("CODECO: {e}"))))?;
            let entries = msg.entries.clone();
            let count = entries.len();
            db(&state.store, move |store| {
                for e in entries {
                    if matches!(e.event, portos_edi::codeco::GateEvent::GateIn) {
                        portos_store::yard::mark_gate_in(store, &e.container, e.at)?;
                    } else {
                        portos_store::yard::mark_gate_out(store, &e.container, e.at)?;
                        portos_store::yard::release_container_slot(store, &e.container)?;
                    }
                }
                Ok(())
            })
            .await
            .map_err(|e| problem(&state, &ctx, e))?;
            respond_edi(&state, ctx, auth, "CODECO", count, None).await
        }
        "COARRI" => {
            let msg = CoarriMessage::parse(&interchange)
                .map_err(|e| problem(&state, &ctx, CoreError::Validation(format!("COARRI: {e}"))))?;
            let entries = msg.entries.clone();
            let count = entries.len();
            db(&state.store, move |store| {
                for e in entries {
                    // Confirm the move as a job record (crane reports — F-13/F-32).
                    let job = portos_core::work::Job {
                        id: portos_core::ids::JobId::generate(),
                        kind: match e.move_type {
                            JobKind::Discharge | JobKind::Load | JobKind::Shift => e.move_type,
                        },
                        call: None,
                        container: Some(e.container),
                        assigned_to: None,
                        priority: 5,
                        status: portos_core::work::JobStatus::Done,
                        created_at: e.started_at,
                        assigned_at: Some(e.started_at),
                        completed_at: e.finished_at,
                        issue: None,
                    };
                    portos_store::work::create_job(store, &job)?;
                    let mv = portos_core::work::MoveRecord {
                        id: portos_core::ids::MoveId::generate(),
                        job: job.id,
                        equipment: portos_core::ids::EquipmentId::from_uuid(uuid::Uuid::nil()),
                        container: job.container.clone().unwrap(),
                        kind: e.move_type,
                        started_at: e.started_at,
                        finished_at: e.finished_at.unwrap_or(e.started_at),
                        success: e.status == "1",
                    };
                    portos_store::work::record_move(store, &mv)?;
                }
                Ok(())
            })
            .await
            .map_err(|e| problem(&state, &ctx, e))?;
            respond_edi(&state, ctx, auth, "COARRI", count, msg.vessel_name).await
        }
        "BAPLIE" => {
            let msg = BaplieMessage::parse(&interchange)
                .map_err(|e| problem(&state, &ctx, CoreError::Validation(format!("BAPLIE: {e}"))))?;
            let entries = msg.entries.clone();
            let count = entries.len();
            // Stowage plans are ingested as container master data + state.
            db(&state.store, move |store| {
                for e in entries {
                    let ct = Container {
                        number: e.container,
                        size_type: e.size_type,
                        weight_kg: e.weight_kg.unwrap_or(0),
                        imdg_class: None,
                        reefer_setpoint_c: None,
                        over_gauge: false,
                        seal_number: None,
                        customer: None,
                        pol: msg.port.clone(),
                        pod: None,
                    };
                    portos_store::yard::upsert_container(store, &ct)?;
                    portos_store::yard::set_container_state(store, &ct.number, ContainerState::OnVessel)?;
                }
                Ok(())
            })
            .await
            .map_err(|e| problem(&state, &ctx, e))?;
            respond_edi(&state, ctx, auth, "BAPLIE", count, msg.vessel_name).await
        }
        other => Err(problem(
            &state,
            &ctx,
            CoreError::Validation(format!(
                "unsupported message type {other:?}; expected COPARN, CODECO, COARRI or BAPLIE"
            )),
        )),
    }
}

async fn respond_edi(
    state: &AppState,
    ctx: LocaleCtx,
    auth: AuthContext,
    message_type: &str,
    count: usize,
    vessel: Option<String>,
) -> ApiResult<serde_json::Value> {
    crate::events::publish(
        state,
        portos_core::events::DomainEventKind::EdiMessageReceived {
            message_type: message_type.to_string(),
            containers: count,
        },
    );
    crate::audit(state, &auth, "edi.import", "edi_message", message_type).await;
    let msg = state.t(
        ctx.locale,
        "edi.received",
        &Args::new().arg("message_type", message_type).arg("count", count.to_string()),
    );
    Ok(Json(serde_json::json!({
        "message_type": message_type,
        "containers": count,
        "vessel": vessel,
        "message": msg,
    })))
}

#[derive(Deserialize)]
pub struct ExportBody {
    pub message_type: String,
    /// For BAPLIE export: containers with optional positions.
    pub containers: Vec<ExportContainer>,
    pub vessel_name: Option<String>,
    pub voyage: Option<String>,
    pub port: Option<String>,
}

#[derive(Deserialize)]
pub struct ExportContainer {
    pub number: String,
    pub size_type: String,
    pub full: Option<bool>,
    pub weight_kg: Option<u32>,
    /// "bay:row:tier" per ISO position encoding.
    pub position: Option<String>,
}

/// POST /v1/edi/export — serialize an outbound EDIFACT message (I-2).
pub async fn export_message(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<ExportBody>,
) -> ApiResult<serde_json::Value> {
    auth.require("edi.export").map_err(|e| problem(&state, &ctx, e))?;
    let message_type = body.message_type.trim().to_ascii_uppercase();
    if message_type != "BAPLIE" {
        return Err(problem(&state, &ctx, CoreError::Validation("only BAPLIE export is supported in Phase 1".into())));
    }
    let mut entries = Vec::new();
    for c in &body.containers {
        let position = match &c.position {
            Some(p) => Some(portos_edi::baplie::StowPosition::parse_triple(p).map_err(|e| {
                problem(&state, &ctx, CoreError::Validation(format!("position: {e}")))
            })?),
            None => None,
        };
        entries.push(portos_edi::baplie::BaplieEntry {
            container: ContainerNumber::parse(&c.number)
                .map_err(|e| problem(&state, &ctx, CoreError::Validation(format!("container: {e}"))))?,
            size_type: portos_core::container::IsoSizeType::parse(&c.size_type)
                .map_err(|e| problem(&state, &ctx, CoreError::Validation(format!("size_type: {e}"))))?,
            full: c.full.unwrap_or(true),
            weight_kg: c.weight_kg,
            position,
        });
    }
    let msg = BaplieMessage {
        message_ref: format!("BAP{}", chrono::Utc::now().format("%Y%m%d%H%M%S")),
        vessel_name: body.vessel_name,
        voyage: body.voyage,
        port: body.port,
        entries,
    };
    let raw = msg.emit("IRBNDCT:ZZZ", "LINE:ZZZ", chrono::Utc::now());
    crate::audit(&state, &auth, "edi.export", "edi_message", "BAPLIE").await;
    Ok(Json(serde_json::json!({
        "message_type": "BAPLIE",
        "containers": msg.entries.len(),
        "raw": raw,
        "message": state.t(ctx.locale, "edi.parsed", &Args::new()),
    })))
}
