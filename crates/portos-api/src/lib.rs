//! PortOS API library: modules are exposed here so integration tests and
//! future crates (gRPC adapters, workers) can reuse the server pieces.

pub mod auth;
pub mod config;
pub mod db;
pub mod docs;
pub mod error;
pub mod events;
pub mod http_post;
pub mod locale;
pub mod metrics;
pub mod middleware_stack;
pub mod ratelimit;
pub mod routes;
pub mod seed;
pub mod state;

use portos_core::CoreResult;

/// Append an audit record for a user action (F-40). Fire-and-forget.
pub async fn audit(
    state: &state::AppState,
    actor: &auth::AuthContext,
    action: &str,
    entity_type: &str,
    entity_id: &str,
) {
    let mut record = portos_core::audit::AuditRecord::system(action, entity_type, entity_id);
    record.actor = actor.username.clone();
    let store = state.store.clone();
    let _ = tokio::task::spawn_blocking(move || portos_store::misc::append_audit(&store, &record))
        .await;
}

/// Convenience for tests/tools: build a test-ready state over a temp DB.
pub fn state_for_db(cfg: config::Config) -> CoreResult<state::AppState> {
    let store = portos_store::Store::open(&cfg.db_path)?;
    Ok(state::AppState::new(store, cfg))
}
