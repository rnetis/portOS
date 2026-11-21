//! Router assembly: public routes, protected routes, middleware stack.

pub mod admin;
pub mod auth_routes;
pub mod billing;
pub mod edi;
pub mod gate;
pub mod health;
pub mod i18n_routes;
pub mod kpi;
pub mod vessels;
pub mod work;
pub mod yard;

use crate::state::AppState;
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use tower_http::trace::TraceLayer;

/// The complete application router.
pub fn build(state: AppState) -> Router {
    let public = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/version", get(health::version))
        .route("/metrics", get(health::metrics))
        .route("/api-docs", get(health::api_docs))
        .route("/api-docs/openapi.json", get(health::openapi_json))
        .route("/v1/auth/login", post(auth_routes::login))
        .route("/v1/i18n/catalog", get(i18n_routes::catalog))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::locale::locale_middleware,
        ));

    let protected = Router::new()
        // auth
        .route("/v1/auth/me", get(auth_routes::me))
        // vessels & calls (F-1, F-3)
        .route(
            "/v1/vessels",
            post(vessels::create_vessel).get(vessels::list_vessels),
        )
        .route("/v1/vessels/{id}", get(vessels::get_vessel))
        .route(
            "/v1/vessel-calls",
            post(vessels::create_call).get(vessels::list_calls),
        )
        .route("/v1/vessel-calls/{id}", get(vessels::get_call))
        .route("/v1/vessel-calls/{id}/status", post(vessels::change_status))
        .route("/v1/vessel-calls/{id}/eta", post(vessels::update_eta))
        // berths & allocation (F-2, F-5)
        .route(
            "/v1/berths",
            post(vessels::create_berth).get(vessels::list_berths),
        )
        .route(
            "/v1/vessel-calls/{id}/berth-request",
            post(vessels::create_berth_request),
        )
        .route(
            "/v1/vessel-calls/{id}/berth-suggest",
            post(vessels::suggest_berths),
        )
        .route(
            "/v1/vessel-calls/{id}/berth-allocate",
            post(vessels::allocate_berth),
        )
        // yard (F-7..F-12)
        .route(
            "/v1/yard/blocks",
            post(yard::create_block).get(yard::list_blocks),
        )
        .route("/v1/yard/blocks/{id}/snapshot", get(yard::block_snapshot))
        .route(
            "/v1/yard/containers",
            post(yard::upsert_container).get(yard::list_containers),
        )
        .route("/v1/yard/containers/{number}", get(yard::get_container))
        .route(
            "/v1/yard/containers/{number}/place",
            post(yard::place_container),
        )
        .route("/v1/yard/containers/{number}/hold", post(yard::place_hold))
        .route("/v1/yard/holds/{id}/release", post(yard::release_hold))
        .route(
            "/v1/yard/containers/{number}/reefer-reading",
            post(yard::reefer_reading),
        )
        // gate (F-17..F-21)
        .route(
            "/v1/gate/appointments",
            post(gate::book_appointment).get(gate::list_appointments),
        )
        .route(
            "/v1/gate/appointments/{reference}/cancel",
            post(gate::cancel_appointment),
        )
        .route("/v1/gate/transactions", get(gate::list_transactions))
        .route("/v1/gate/transactions/start", post(gate::start_transaction))
        .route("/v1/gate/transactions/{id}/step", post(gate::apply_step))
        .route("/v1/gate/transactions/{id}", get(gate::get_transaction))
        .route("/v1/gate/queue", get(gate::queue_status))
        .route("/v1/gate/passes/{pass_no}", get(gate::get_pass))
        // equipment & jobs (F-13..F-15)
        .route(
            "/v1/work/equipment",
            post(work::create_equipment).get(work::list_equipment),
        )
        .route(
            "/v1/work/equipment/{id}/status",
            post(work::set_equipment_status),
        )
        .route("/v1/work/jobs", post(work::create_job).get(work::list_jobs))
        .route("/v1/work/jobs/{id}/dispatch", post(work::dispatch_job))
        .route("/v1/work/jobs/{id}/complete", post(work::complete_job))
        // billing (F-27..F-30)
        .route(
            "/v1/billing/customers",
            post(billing::create_customer).get(billing::list_customers),
        )
        .route(
            "/v1/billing/tariffs",
            post(billing::upsert_tariff).get(billing::list_tariffs),
        )
        .route("/v1/billing/fx", post(billing::set_fx))
        .route("/v1/billing/invoices", get(billing::list_invoices))
        .route(
            "/v1/billing/invoices/generate",
            post(billing::generate_invoice),
        )
        .route("/v1/billing/invoices/{number}", get(billing::get_invoice))
        .route(
            "/v1/billing/invoices/{number}/payments",
            post(billing::record_payment),
        )
        // KPIs (F-31, F-32)
        .route("/v1/kpi", get(kpi::dashboard))
        // EDI (I-2)
        .route("/v1/edi/import", post(edi::import_message))
        .route("/v1/edi/export", post(edi::export_message))
        // i18n preferences (L-1)
        .route("/v1/i18n/preferences", post(auth_routes::set_preferences))
        .route("/v1/i18n/format-demo", get(i18n_routes::format_demo))
        // events (F-26)
        .route("/v1/events/stream", get(crate::events::event_stream))
        // admin (F-40, F-44, F-46)
        .route(
            "/v1/admin/users",
            post(admin::create_user).get(admin::list_users),
        )
        .route("/v1/admin/audit", get(admin::query_audit))
        .route(
            "/v1/admin/webhooks",
            post(admin::create_webhook).get(admin::list_webhooks),
        )
        .route(
            "/v1/admin/incidents",
            post(admin::create_incident).get(admin::list_incidents),
        )
        .route("/v1/admin/config", get(admin::effective_config))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::locale::locale_middleware,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::auth::auth_middleware,
        ));

    let body_limit = state.config.body_limit_bytes;

    let cors = if state.config.cors_origins.iter().any(|o| o == "*") {
        tower_http::cors::CorsLayer::permissive()
    } else {
        let origins: Vec<_> = state
            .config
            .cors_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        // Browsers preflight non-simple requests; the allowed origin list
        // alone is not enough for a working cross-origin API.
        tower_http::cors::CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any)
    };

    Router::new()
        .merge(public)
        .merge(protected)
        .layer(middleware::from_fn(crate::middleware_stack::http_metrics))
        .layer(middleware::from_fn(
            crate::middleware_stack::security_headers,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::middleware_stack::rate_limit,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::middleware_stack::timeout,
        ))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(body_limit))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        // Outermost: every response, including 429/504, carries a request id.
        .layer(middleware::from_fn(crate::middleware_stack::request_id))
        .with_state(state)
}
