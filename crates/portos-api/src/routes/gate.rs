//! Gate endpoints: appointments, transactions, passes, queue visibility
//! (PRD F-17..F-21).

use axum::extract::{Path, Query, State};
use axum::Extension;
use axum::Json;
use portos_core::container::ContainerNumber;
use portos_core::gate::{
    AppointmentStatus, GateDirection, GatePass, GateStepKind, GateTransaction, TruckAppointment,
};
use portos_core::ids::AppointmentId;
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
pub struct BookAppointment {
    pub driver_name: String,
    pub driver_license: String,
    pub plate: String,
    pub container: Option<String>,
    pub direction: String,
    pub slot_from: String,
    pub slot_to: String,
}

fn parse_dt(s: &str) -> CoreResult<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&chrono::Utc))
        .map_err(|_| CoreError::Validation(format!("{s:?} must be RFC 3339")))
}

/// POST /v1/gate/appointments — truck appointment booking (F-17).
pub async fn book_appointment(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<BookAppointment>,
) -> ApiResult<serde_json::Value> {
    auth.require("gate.manage")
        .map_err(|e| problem(&state, &ctx, e))?;
    let direction: GateDirection = serde_json::from_str(&format!("\"{}\"", body.direction))
        .map_err(|_| CoreError::Validation(format!("unknown direction {}", body.direction)))
        .map_err(|e| problem(&state, &ctx, e))?;
    let slot_from = parse_dt(&body.slot_from).map_err(|e| problem(&state, &ctx, e))?;
    let slot_to = parse_dt(&body.slot_to).map_err(|e| problem(&state, &ctx, e))?;
    if slot_to <= slot_from {
        return Err(problem(
            &state,
            &ctx,
            CoreError::Validation("slot_to must be after slot_from".into()),
        ));
    }
    let container = match &body.container {
        Some(c) => Some(ContainerNumber::parse(c).map_err(|e| problem(&state, &ctx, e))?),
        None => None,
    };
    let seq = db(&state.store, |store| {
        portos_store::gate::count_appointments(store)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    let reference = format!("TA-{}-{:06}", chrono::Utc::now().format("%Y"), seq + 1);
    let reference_for_appt = reference.clone();
    let appt = TruckAppointment {
        id: AppointmentId::generate(),
        reference: reference_for_appt,
        trucking_company: None,
        driver_name: body.driver_name,
        driver_license: body.driver_license,
        plate: body.plate,
        container,
        direction,
        slot_from,
        slot_to,
        status: AppointmentStatus::Booked,
        created_at: chrono::Utc::now(),
    };
    let a = appt.clone();
    db(&state.store, move |store| {
        portos_store::gate::create_appointment(store, &a)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::events::publish(
        &state,
        portos_core::events::DomainEventKind::AppointmentBooked {
            appointment: appt.id,
            reference: reference.clone(),
            slot_from,
        },
    );
    crate::audit(
        &state,
        &auth,
        "gate.appointment.book",
        "truck_appointment",
        &reference,
    )
    .await;
    let slot_display =
        portos_i18n::formats::datetime_display(ctx.locale, ctx.calendar, ctx.numerals, slot_from);
    let msg = state.t(
        ctx.locale,
        "gate.appointment.booked",
        &Args::new()
            .arg("reference", &reference)
            .arg("slot_from", &slot_display),
    );
    Ok(Json(
        serde_json::json!({ "appointment": appt, "message": msg }),
    ))
}

/// GET /v1/gate/appointments?from=&to= — slot overview (F-17/F-20).
pub async fn list_appointments(
    State(state): State<AppState>,
    Extension(_ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let now = chrono::Utc::now();
    let from = q
        .get("from")
        .and_then(|s| parse_dt(s).ok())
        .unwrap_or(now - chrono::Duration::hours(12));
    let to = q
        .get("to")
        .and_then(|s| parse_dt(s).ok())
        .unwrap_or(now + chrono::Duration::hours(24));
    let appts = db(&state.store, move |store| {
        portos_store::gate::list_appointments_in_window(store, from, to)
    })
    .await
    .map_err(|e| problem(&state, &_ctx, e))?;
    Ok(Json(serde_json::json!({ "appointments": appts })))
}

/// POST /v1/gate/appointments/{reference}/cancel
pub async fn cancel_appointment(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(reference): Path<String>,
) -> ApiResult<serde_json::Value> {
    auth.require("gate.manage")
        .map_err(|e| problem(&state, &ctx, e))?;
    let reference_lookup = reference.clone();
    let appt = db(&state.store, move |store| {
        portos_store::gate::get_appointment_by_reference(store, &reference_lookup)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    db(&state.store, move |store| {
        portos_store::gate::update_appointment_status(store, appt.id, AppointmentStatus::Cancelled)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(
        &state,
        &auth,
        "gate.appointment.cancel",
        "truck_appointment",
        &reference,
    )
    .await;
    let msg = state.t(
        ctx.locale,
        "gate.appointment.cancelled",
        &Args::new().arg("reference", &reference),
    );
    Ok(Json(
        serde_json::json!({ "reference": reference, "message": msg }),
    ))
}

#[derive(Deserialize)]
pub struct StartTx {
    pub direction: String,
    pub appointment_reference: Option<String>,
    pub container: Option<String>,
    pub plate: String,
    pub driver_license: String,
    pub gate_lane: String,
}

/// POST /v1/gate/transactions/start — begins the gate workflow; honors an
/// `Idempotency-Key` header so kiosk retries never double-process (§6.2).
pub async fn start_transaction(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    headers: axum::http::HeaderMap,
    Json(body): Json<StartTx>,
) -> ApiResult<serde_json::Value> {
    auth.require("gate.operate")
        .map_err(|e| problem(&state, &ctx, e))?;
    let direction: GateDirection = serde_json::from_str(&format!("\"{}\"", body.direction))
        .map_err(|_| CoreError::Validation(format!("unknown direction {}", body.direction)))
        .map_err(|e| problem(&state, &ctx, e))?;
    let idempotency_key = headers
        .get("idempotency-key")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let appointment_reference = body.appointment_reference.clone();
    let (appointment_id, expected_container) = match appointment_reference {
        Some(reference) => {
            let reference_lookup = reference.clone();
            let appt = db(&state.store, move |store| {
                portos_store::gate::get_appointment_by_reference(store, &reference_lookup)
            })
            .await
            .map_err(|e| problem(&state, &ctx, e))?;
            if appt.status != AppointmentStatus::Booked {
                return Err(problem(
                    &state,
                    &ctx,
                    CoreError::Conflict(format!(
                        "appointment {reference} is {:?}, not Booked",
                        appt.status
                    )),
                ));
            }
            (Some(appt.id), appt.container.clone())
        }
        None => (None, None),
    };
    let expected = match (&body.container, &expected_container) {
        (Some(c), _) => Some(ContainerNumber::parse(c).map_err(|e| problem(&state, &ctx, e))?),
        (None, e) => e.clone(),
    };

    let tx = GateTransaction::start(
        direction,
        appointment_id,
        expected,
        &body.plate,
        &body.driver_license,
        &body.gate_lane,
        chrono::Utc::now(),
    );
    let t = tx.clone();
    let key = idempotency_key.clone();
    let appt_to_check_in = appointment_id;
    let stored = db(&state.store, move |store| {
        portos_store::gate::create_gate_tx(store, &t, key.as_deref())
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    // A truck that has started the gate workflow has checked in.
    if let Some(appt_id) = appt_to_check_in {
        let _ = db(&state.store, move |store| {
            portos_store::gate::update_appointment_status(
                store,
                appt_id,
                AppointmentStatus::CheckedIn,
            )
        })
        .await;
    }
    crate::metrics::METRICS.inc_by_one("portos_gate_tx_started_total");
    crate::audit(
        &state,
        &auth,
        "gate.tx.start",
        "gate_transaction",
        &stored.id.to_string(),
    )
    .await;
    let msg = state.t(
        ctx.locale,
        "gate.tx.started",
        &Args::new()
            .arg("plate", &body.plate)
            .arg("lane", &body.gate_lane),
    );
    Ok(Json(serde_json::json!({
        "transaction": stored,
        "next_step": portos_core::gate::expected_steps(stored.direction).first(),
        "message": msg,
    })))
}

#[derive(Deserialize)]
pub struct StepRequest {
    pub kind: String,
    pub ok: bool,
    pub detail: Option<String>,
    /// OCR container number (required for the ocr-scan step).
    pub ocr_container: Option<String>,
    pub seal_number: Option<String>,
}

/// POST /v1/gate/transactions/{id}/step — apply one workflow step
/// (F-18); on completion issues a digital gate pass + EIR (F-19) and
/// publishes events.
pub async fn apply_step(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(body): Json<StepRequest>,
) -> ApiResult<serde_json::Value> {
    auth.require("gate.operate")
        .map_err(|e| problem(&state, &ctx, e))?;
    let tid = id
        .parse::<uuid::Uuid>()
        .map(portos_core::ids::GateTxId::from_uuid)
        .map_err(|_| CoreError::Validation(format!("invalid transaction id {id:?}")))
        .map_err(|e| problem(&state, &ctx, e))?;
    let kind: GateStepKind = serde_json::from_str(&format!("\"{}\"", body.kind))
        .map_err(|_| CoreError::Validation(format!("unknown step kind {}", body.kind)))
        .map_err(|e| problem(&state, &ctx, e))?;
    let mut tx = db(&state.store, move |store| {
        portos_store::gate::get_gate_tx(store, tid)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;

    if kind == GateStepKind::OcrScan {
        if let Some(text) = &body.ocr_container {
            let scanned = ContainerNumber::parse(text)
                .map_err(|e| problem(&state, &ctx, CoreError::Validation(format!("OCR: {e}"))))?;
            tx.set_ocr(Some(scanned));
        } else {
            tx.set_ocr(None);
        }
    }
    if let Some(seal) = &body.seal_number {
        tx.set_seal(seal);
    }
    tx.apply(kind, body.ok, body.detail.clone(), chrono::Utc::now())
        .map_err(|e| problem(&state, &ctx, e))?;
    let t = tx.clone();
    db(&state.store, move |store| {
        portos_store::gate::update_gate_tx(store, &t)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;

    // Completed gate-in/out → pass, EIR, state updates and events (F-19).
    let mut issued: Option<serde_json::Value> = None;
    if tx.status == portos_core::gate::GateStatus::Completed {
        let seq = db(&state.store, |store| {
            portos_store::gate::count_gate_tx(store)
        })
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
        let pass = GatePass::issue(&tx, seq as u64 + 1);
        let p = pass.clone();
        db(&state.store, move |store| {
            portos_store::gate::save_gate_pass(store, &p)
        })
        .await
        .map_err(|e| problem(&state, &ctx, e))?;

        if let Some(container) = &tx.expected_container {
            let cn = container.clone();
            let eir = portos_core::gate::EquipmentInterchangeReport::create(
                &tx,
                cn.clone(),
                seq as u64 + 1,
            );
            let e = eir.clone();
            db(&state.store, move |store| {
                portos_store::gate::save_eir(store, &e)
            })
            .await
            .map_err(|e| problem(&state, &ctx, e))?;
            issued = Some(serde_json::json!({
                "pass_no": pass.pass_no,
                "eir_no": eir.eir_no,
            }));
            if tx.direction.is_gate_in() {
                let cn_in = container.clone();
                db(&state.store, move |store| {
                    portos_store::yard::mark_gate_in(store, &cn_in, chrono::Utc::now())
                })
                .await
                .map_err(|e| problem(&state, &ctx, e))?;
                crate::events::publish(
                    &state,
                    portos_core::events::DomainEventKind::ContainerGateIn {
                        gate_tx: tx.id,
                        container: Some(container.clone()),
                        direction: tx.direction.to_string(),
                    },
                );
            } else {
                let cn_out = container.clone();
                let cn_release = container.clone();
                db(&state.store, move |store| {
                    portos_store::yard::mark_gate_out(store, &cn_out, chrono::Utc::now())
                })
                .await
                .map_err(|e| problem(&state, &ctx, e))?;
                db(&state.store, move |store| {
                    portos_store::yard::release_container_slot(store, &cn_release)
                })
                .await
                .map_err(|e| problem(&state, &ctx, e))?;
                crate::events::publish(
                    &state,
                    portos_core::events::DomainEventKind::ContainerGateOut {
                        gate_tx: tx.id,
                        container: Some(container.clone()),
                    },
                );
            }
        } else {
            crate::events::publish(
                &state,
                portos_core::events::DomainEventKind::ContainerGateIn {
                    gate_tx: tx.id,
                    container: None,
                    direction: tx.direction.to_string(),
                },
            );
        }
        // Settle the appointment that admitted this truck (F-17/F-20).
        if let Some(appt_id) = tx.appointment {
            let _ = db(&state.store, move |store| {
                portos_store::gate::update_appointment_status(
                    store,
                    appt_id,
                    AppointmentStatus::Completed,
                )
            })
            .await;
        }
        crate::metrics::METRICS.inc_by_one("portos_gate_tx_completed_total");
        if let Some(elapsed) = tx.elapsed_seconds() {
            // Gate transactions under 5s target (§6.1).
            crate::metrics::METRICS.observe("portos_gate_tx_duration_seconds", elapsed);
        }
        crate::audit(
            &state,
            &auth,
            "gate.tx.complete",
            "gate_transaction",
            &tx.id.to_string(),
        )
        .await;
    }

    Ok(Json(serde_json::json!({
        "transaction": tx,
        "issued": issued,
        "elapsed_seconds": tx.elapsed_seconds(),
    })))
}

/// GET /v1/gate/transactions/{id}
pub async fn get_transaction(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Path(id): Path<String>,
) -> ApiResult<serde_json::Value> {
    let tid = id
        .parse::<uuid::Uuid>()
        .map(portos_core::ids::GateTxId::from_uuid)
        .map_err(|_| CoreError::Validation(format!("invalid transaction id {id:?}")))
        .map_err(|e| problem(&state, &ctx, e))?;
    let tx = db(&state.store, move |store| {
        portos_store::gate::get_gate_tx(store, tid)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    Ok(Json(serde_json::to_value(&tx).unwrap_or_default()))
}

/// GET /v1/gate/transactions?from=&to=&limit=
pub async fn list_transactions(
    State(state): State<AppState>,
    Extension(_ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let now = chrono::Utc::now();
    let from = q
        .get("from")
        .and_then(|s| parse_dt(s).ok())
        .unwrap_or(now - chrono::Duration::hours(24));
    let to = q.get("to").and_then(|s| parse_dt(s).ok()).unwrap_or(now);
    let limit: u32 = q
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(100)
        .min(1000);
    let txs = db(&state.store, move |store| {
        portos_store::gate::list_recent_gate_tx(store, from, to, limit)
    })
    .await
    .map_err(|e| problem(&state, &_ctx, e))?;
    Ok(Json(serde_json::json!({ "transactions": txs })))
}

/// GET /v1/gate/queue — real-time queue visibility for drivers and
/// dispatchers (F-20).
pub async fn queue_status(
    State(state): State<AppState>,
    Extension(_ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
) -> ApiResult<serde_json::Value> {
    let now = chrono::Utc::now();
    let (appts, in_progress) = db(&state.store, move |store| {
        let appts = portos_store::gate::list_appointments_in_window(
            store,
            now - chrono::Duration::hours(1),
            now + chrono::Duration::hours(6),
        )?;
        let txs = portos_store::gate::list_recent_gate_tx(
            store,
            now - chrono::Duration::hours(1),
            now,
            200,
        )?;
        let in_progress = txs
            .iter()
            .filter(|t| t.status == portos_core::gate::GateStatus::InProgress)
            .count();
        Ok((appts, in_progress))
    })
    .await
    .map_err(|e| problem(&state, &_ctx, e))?;
    let booked = appts
        .iter()
        .filter(|a| a.status == AppointmentStatus::Booked)
        .count();
    Ok(Json(serde_json::json!({
        "now": now.to_rfc3339(),
        "appointments_in_window": appts.len(),
        "booked_waiting": booked,
        "transactions_in_progress": in_progress,
    })))
}

/// GET /v1/gate/passes/{pass_no} — digital gate pass lookup (F-19).
pub async fn get_pass(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Path(pass_no): Path<String>,
) -> ApiResult<serde_json::Value> {
    let pass = db(&state.store, move |store| {
        portos_store::gate::get_gate_pass(store, &pass_no)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    let title = state.t(ctx.locale, "gate.pass.title", &Args::new());
    Ok(Json(serde_json::json!({ "title": title, "pass": pass })))
}
