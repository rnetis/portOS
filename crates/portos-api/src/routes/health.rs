//! Health probes, version, metrics and API documentation endpoints.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use portos_core::VERSION;

use crate::state::AppState;

/// Liveness: process is up.
pub async fn healthz() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok", "version": VERSION }))
}

/// Readiness: migrations applied, event bus alive — for orchestrators (§6.2).
pub async fn readyz(State(state): State<AppState>) -> Response {
    // Readiness must reflect the database too: a process that cannot serve
    // queries should be taken out of the load-balancer pool (§6.2).
    let store = state.store.clone();
    let db_ok = tokio::task::spawn_blocking(move || store.ping())
        .await
        .map(|r| r.is_ok())
        .unwrap_or(false);
    let ready = state.is_ready() && db_ok;
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        Json(serde_json::json!({
            "status": if ready { "ready" } else { "starting" },
            "database": if db_ok { "ok" } else { "unavailable" },
            "uptime_secs": state.uptime_secs(),
        })),
    )
        .into_response()
}

/// Build metadata.
pub async fn version() -> impl IntoResponse {
    Json(serde_json::json!({
        "name": "portosd",
        "version": VERSION,
        "event_schema": portos_core::EVENT_SCHEMA_VERSION,
    }))
}

/// Prometheus scrape endpoint (§6.5).
pub async fn metrics() -> Response {
    let body = crate::metrics::METRICS.render();
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
        .into_response()
}

/// Human-readable API index.
pub async fn api_docs() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        crate::docs::INDEX_HTML,
    )
}

/// Machine-readable OpenAPI 3.0 description (I-1).
pub async fn openapi_json() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
        crate::docs::OPENAPI_JSON,
    )
}
