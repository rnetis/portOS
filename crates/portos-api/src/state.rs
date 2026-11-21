//! Shared application state.

use portos_i18n::CatalogSet;
use portos_store::Store;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;
use tokio::sync::broadcast;

use crate::config::Config;
use crate::events::WebhookQueue;

#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub catalogs: Arc<RwLock<CatalogSet>>,
    pub config: Arc<Config>,
    /// Domain event bus for SSE subscribers (F-26).
    pub events: broadcast::Sender<portos_core::events::DomainEvent>,
    /// Outbound webhook queue driven by the dispatcher worker (I-1).
    pub webhooks: Arc<WebhookQueue>,
    /// Global per-IP request limiter.
    pub global_limiter: Arc<crate::ratelimit::RateLimiter>,
    /// Stricter limiter for login attempts (brute-force protection).
    pub login_limiter: Arc<crate::ratelimit::RateLimiter>,
    pub started_at: Instant,
    pub ready: Arc<AtomicBool>,
}

impl AppState {
    /// Build the shared state from a store and config.
    #[must_use]
    pub fn new(store: Store, config: Config) -> Self {
        let (events, _) = broadcast::channel(1024);
        let global_limiter =
            Arc::new(crate::ratelimit::RateLimiter::new(config.rate_limit_rps, (config.rate_limit_rps * 2.0).ceil() as u32));
        let login_per_sec = config.login_rate_per_min as f64 / 60.0;
        let login_limiter =
            Arc::new(crate::ratelimit::RateLimiter::new(login_per_sec, config.login_rate_per_min.min(20)));
        Self {
            store,
            catalogs: Arc::new(RwLock::new(CatalogSet::embedded())),
            config: Arc::new(config),
            events,
            webhooks: Arc::new(WebhookQueue::default()),
            global_limiter,
            login_limiter,
            started_at: Instant::now(),
            ready: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Translate with the current catalogs (L-8).
    pub fn t(&self, locale: portos_i18n::Locale, key: &str, args: &portos_i18n::Args) -> String {
        self.catalogs
            .read()
            .map(|c| c.t(locale, key, args))
            .unwrap_or_else(|_| key.to_string())
    }

    /// Reload catalogs, applying override directories (L-10).
    pub fn reload_catalogs(&self) -> Result<Vec<String>, String> {
        let mut set = CatalogSet::embedded();
        let mut warnings = Vec::new();
        for dir in &self.config.i18n_extra_dirs {
            match set.load_overrides(dir) {
                Ok(w) => warnings.extend(w),
                Err(e) => warnings.push(format!("{}: {e}", dir.display())),
            }
        }
        if let Ok(mut guard) = self.catalogs.write() {
            *guard = set;
        }
        Ok(warnings)
    }

    pub fn uptime_secs(&self) -> u64 {
        self.started_at.elapsed().as_secs()
    }

    pub fn set_ready(&self, ready: bool) {
        self.ready.store(ready, Ordering::Relaxed);
    }

    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }
}
