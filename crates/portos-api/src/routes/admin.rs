//! Admin endpoints: users, audit trail, webhooks, incidents, config
//! (PRD F-38, F-40, F-44, F-46).

use axum::extract::{Query, State};
use axum::Extension;
use axum::Json;
use portos_core::ids::IncidentId;
use portos_core::CoreError;
use portos_i18n::{Args, Calendar, Locale, Numerals};
use serde::Deserialize;
use std::collections::HashMap;

use crate::auth::{hash_password, AuthContext, Role};
use crate::db::db;
use crate::error::{problem, ApiResult};
use crate::locale::LocaleCtx;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateUser {
    pub username: String,
    pub password: String,
    pub display_name: String,
    pub role: String,
    pub locale: Option<String>,
    pub calendar: Option<String>,
    pub numerals: Option<String>,
}

/// POST /v1/admin/users — create an account with a role (F-44).
pub async fn create_user(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<CreateUser>,
) -> ApiResult<serde_json::Value> {
    auth.require("admin.manage")
        .map_err(|e| problem(&state, &ctx, e))?;
    let username = body.username.trim().to_lowercase();
    if username.len() < 3
        || !username
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(problem(
            &state,
            &ctx,
            CoreError::Validation("username must be 3+ chars of [a-z0-9_]".into()),
        ));
    }
    if body.password.len() < 10 {
        return Err(problem(
            &state,
            &ctx,
            CoreError::Validation("password must be at least 10 characters".into()),
        ));
    }
    let role = Role::parse(&body.role).map_err(|e| problem(&state, &ctx, e))?;
    // Argon2 hashing is CPU-bound; never run it on the async runtime.
    let password = body.password.clone();
    let hash = tokio::task::spawn_blocking(move || hash_password(&password))
        .await
        .map_err(|e| problem(&state, &ctx, CoreError::Internal(format!("hash task: {e}"))))?
        .map_err(|e| problem(&state, &ctx, e))?;
    let locale = body
        .locale
        .as_deref()
        .and_then(Locale::parse)
        .unwrap_or(ctx.locale);
    let calendar = body
        .calendar
        .as_deref()
        .and_then(Calendar::parse)
        .unwrap_or(portos_i18n::defaults_for(locale).0);
    let numerals = body
        .numerals
        .as_deref()
        .and_then(Numerals::parse)
        .unwrap_or(portos_i18n::defaults_for(locale).1);
    let display = body.display_name.clone();
    let created = db(&state.store, move |store| {
        portos_store::users::create(
            store,
            &username,
            &hash,
            &display,
            role.as_str(),
            locale,
            calendar,
            numerals,
        )
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    let username = created.username.clone();
    crate::audit(&state, &auth, "admin.user.create", "user", &username).await;
    let msg = state.t(
        ctx.locale,
        "admin.user.created",
        &Args::new()
            .arg("username", &username)
            .arg("role", role.as_str()),
    );
    Ok(Json(serde_json::json!({
        "username": created.username,
        "role": created.role,
        "locale": created.locale.tag(),
        "message": msg,
    })))
}

/// GET /v1/admin/users
pub async fn list_users(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<serde_json::Value> {
    auth.require("admin.manage")
        .map_err(|e| problem(&state, &ctx, e))?;
    let users = db(&state.store, portos_store::users::list)
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
    Ok(Json(serde_json::json!({ "users": users })))
}

/// GET /v1/admin/audit?actor=&entity_type=&from= — full audit trail (F-40).
pub async fn query_audit(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    auth.require("admin.manage")
        .or_else(|_| auth.require("kpi.view"))
        .map_err(|e| problem(&state, &ctx, e))?;
    let actor = q.get("actor").cloned();
    let entity_type = q.get("entity_type").cloned();
    let from = q
        .get("from")
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc));
    let limit: u32 = q
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(100)
        .min(1000);
    let records = db(&state.store, move |store| {
        portos_store::misc::query_audit(
            store,
            actor.as_deref(),
            entity_type.as_deref(),
            from,
            limit,
        )
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    Ok(Json(serde_json::json!({ "records": records })))
}

#[derive(Deserialize)]
pub struct WebhookBody {
    pub url: String,
    /// Comma-separated event types or `*` for all.
    pub events: Option<String>,
}

/// POST /v1/admin/webhooks — subscribe an HTTPS endpoint to domain events
/// (I-1). Deliveries carry an HMAC-SHA256 signature.
pub async fn create_webhook(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<WebhookBody>,
) -> ApiResult<serde_json::Value> {
    auth.require("admin.manage")
        .map_err(|e| problem(&state, &ctx, e))?;
    if !body.url.starts_with("https://") && !body.url.starts_with("http://localhost") {
        return Err(problem(
            &state,
            &ctx,
            CoreError::Validation("webhook URL must be https:// (http only for localhost)".into()),
        ));
    }
    // Generate a strong subscription secret; shown once to the operator.
    let secret = format!("whk_{}", uuid::Uuid::now_v7().simple());
    let url = body.url.clone();
    let events_filter = body.events.clone().unwrap_or_else(|| "*".into());
    let secret_for_db = secret.clone();
    let row = db(&state.store, move |store| {
        portos_store::misc::create_webhook(store, &url, &secret_for_db, &events_filter)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(
        &state,
        &auth,
        "admin.webhook.create",
        "webhook",
        &row.id.to_string(),
    )
    .await;
    let msg = state.t(
        ctx.locale,
        "admin.webhook.created",
        &Args::new().arg("url", &row.url),
    );
    Ok(Json(serde_json::json!({
        "webhook": { "id": row.id.to_string(), "url": row.url, "events": row.events, "active": row.active },
        "secret": secret,
        "message": msg,
    })))
}

/// GET /v1/admin/webhooks
pub async fn list_webhooks(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<serde_json::Value> {
    auth.require("admin.manage")
        .map_err(|e| problem(&state, &ctx, e))?;
    let hooks = db(&state.store, |store| {
        portos_store::misc::list_active_webhooks(store)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    let hooks: Vec<serde_json::Value> = hooks
        .into_iter()
        .map(|h| serde_json::json!({ "id": h.id.to_string(), "url": h.url, "events": h.events, "active": h.active }))
        .collect();
    Ok(Json(serde_json::json!({ "webhooks": hooks })))
}

#[derive(Deserialize)]
pub struct IncidentBody {
    pub kind: String,
    pub severity: String,
    pub location: Option<String>,
    pub description: String,
}

/// POST /v1/admin/incidents — safety incident logging (F-38); publishes
/// `safety.incident_reported`.
pub async fn create_incident(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<IncidentBody>,
) -> ApiResult<serde_json::Value> {
    auth.require("incident.report")
        .map_err(|e| problem(&state, &ctx, e))?;
    let incident = portos_store::misc::IncidentRow {
        id: IncidentId::generate(),
        kind: body.kind,
        severity: body.severity,
        location: body.location,
        description: body.description,
        occurred_at: chrono::Utc::now(),
        reported_by: auth.username.clone(),
        status: "open".into(),
        actions: Vec::new(),
    };
    let i = incident.clone();
    db(&state.store, move |store| {
        portos_store::misc::create_incident(store, &i)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::events::publish(
        &state,
        portos_core::events::DomainEventKind::IncidentReported {
            incident: incident.id,
            severity: incident.severity.clone(),
        },
    );
    crate::audit(
        &state,
        &auth,
        "safety.incident",
        "incident",
        &incident.id.to_string(),
    )
    .await;
    Ok(Json(serde_json::to_value(&incident).unwrap_or_default()))
}

/// GET /v1/admin/incidents?status=open
pub async fn list_incidents(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let status = q.get("status").cloned();
    let limit: u32 = q
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(100)
        .min(1000);
    let incidents = db(&state.store, move |store| {
        portos_store::misc::list_incidents(store, status.as_deref(), limit)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    Ok(Json(serde_json::json!({ "incidents": incidents })))
}

/// GET /v1/admin/config — effective configuration with the secret redacted.
pub async fn effective_config(
    State(state): State<AppState>,
    Extension(_ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
) -> ApiResult<serde_json::Value> {
    auth.require("admin.manage")
        .map_err(|e| problem(&state, &_ctx, e))?;
    Ok(Json(serde_json::json!({
        "config": state.config.redacted_summary(),
        "tenant": state.config.tenant,
        "uptime_secs": state.uptime_secs(),
        "event_schema": portos_core::EVENT_SCHEMA_VERSION,
        "version": portos_core::VERSION,
    })))
}
