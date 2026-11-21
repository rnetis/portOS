//! Blocking-thread-pool helper: every SQLite call runs here so async
//! handlers never block the runtime (§6.1 performance).

use portos_core::{CoreError, CoreResult};
use portos_store::Store;

/// Run a blocking store operation on Tokio's blocking pool.
pub async fn db<T, F>(store: &Store, f: F) -> CoreResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Store) -> CoreResult<T> + Send + 'static,
{
    let store = store.clone();
    tokio::task::spawn_blocking(move || f(&store))
        .await
        .map_err(|e| CoreError::Internal(format!("task join: {e}")))?
}
