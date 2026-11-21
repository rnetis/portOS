//! Runtime configuration from environment variables (12-factor).
//!
//! Every variable has a safe default for local development; production
//! deployments set `PORTOS_JWT_SECRET` explicitly (the server refuses to
//! start with the development default when `PORTOS_REQUIRE_SECRET=1`).

use portos_core::{CoreError, CoreResult};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    /// Listen address, e.g. `0.0.0.0:8080`.
    pub bind_addr: String,
    /// SQLite database file.
    pub db_path: PathBuf,
    /// HMAC secret for JWT signing.
    pub jwt_secret: String,
    /// Access-token lifetime in hours.
    pub jwt_ttl_hours: i64,
    /// Tenant/terminal code stamped into events and audit records (F-46).
    pub tenant: String,
    /// CORS origins (`*` for development).
    pub cors_origins: Vec<String>,
    /// Global request rate limit (requests/second per client IP).
    pub rate_limit_rps: f64,
    /// Login rate limit (attempts/minute per IP) — brute-force protection.
    pub login_rate_per_min: u32,
    /// Request body limit in bytes.
    pub body_limit_bytes: usize,
    /// Default request timeout seconds.
    pub timeout_secs: u64,
    /// Extra i18n override directories (L-10).
    pub i18n_extra_dirs: Vec<PathBuf>,
    /// Default locale for unauthenticated requests.
    pub default_locale: portos_i18n::Locale,
    /// Log format: `json` (default) or `pretty`.
    pub log_format: LogFormat,
    /// Webhook delivery timeout seconds.
    pub webhook_timeout_secs: u64,
    /// Refuse to boot with the default dev JWT secret.
    pub require_secret: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Json,
    Pretty,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| default.to_string())
}

impl Config {
    /// Load from environment with defaults.
    pub fn from_env() -> CoreResult<Config> {
        let default_locale = portos_i18n::Locale::parse(&env_or("PORTOS_DEFAULT_LOCALE", "en"))
            .unwrap_or_default();
        let cors_origins = env_or("PORTOS_CORS_ORIGINS", "*")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let jwt_secret = env_or(
            "PORTOS_JWT_SECRET",
            "dev-only-secret-change-me-in-production-0f3a",
        );
        let cfg = Config {
            bind_addr: env_or("PORTOS_BIND_ADDR", "0.0.0.0:8080"),
            db_path: PathBuf::from(env_or("PORTOS_DB_PATH", "data/portos.db")),
            jwt_secret,
            jwt_ttl_hours: env_or("PORTOS_JWT_TTL_HOURS", "8").parse().map_err(|_| {
                CoreError::Validation("PORTOS_JWT_TTL_HOURS must be an integer".into())
            })?,
            tenant: env_or("PORTOS_TENANT", "IRBND-CT1"),
            cors_origins,
            rate_limit_rps: env_or("PORTOS_RATE_LIMIT_RPS", "100").parse().map_err(|_| {
                CoreError::Validation("PORTOS_RATE_LIMIT_RPS must be a number".into())
            })?,
            login_rate_per_min: env_or("PORTOS_LOGIN_RATE_PER_MIN", "10").parse().map_err(|_| {
                CoreError::Validation("PORTOS_LOGIN_RATE_PER_MIN must be an integer".into())
            })?,
            body_limit_bytes: env_or("PORTOS_BODY_LIMIT_BYTES", "2097152").parse().map_err(|_| {
                CoreError::Validation("PORTOS_BODY_LIMIT_BYTES must be an integer".into())
            })?,
            timeout_secs: env_or("PORTOS_TIMEOUT_SECS", "30").parse().map_err(|_| {
                CoreError::Validation("PORTOS_TIMEOUT_SECS must be an integer".into())
            })?,
            i18n_extra_dirs: std::env::var("PORTOS_I18N_EXTRA_DIRS")
                .unwrap_or_default()
                .split(':')
                .filter(|s| !s.trim().is_empty())
                .map(PathBuf::from)
                .collect(),
            default_locale,
            log_format: match env_or("PORTOS_LOG_FORMAT", "json").as_str() {
                "pretty" => LogFormat::Pretty,
                _ => LogFormat::Json,
            },
            webhook_timeout_secs: env_or("PORTOS_WEBHOOK_TIMEOUT_SECS", "10").parse().map_err(|_| {
                CoreError::Validation("PORTOS_WEBHOOK_TIMEOUT_SECS must be an integer".into())
            })?,
            require_secret: env_or("PORTOS_REQUIRE_SECRET", "0") == "1",
        };
        if cfg.require_secret && cfg.jwt_secret == "dev-only-secret-change-me-in-production-0f3a" {
            return Err(CoreError::Internal(
                "PORTOS_JWT_SECRET must be set when PORTOS_REQUIRE_SECRET=1".into(),
            ));
        }
        Ok(cfg)
    }

    /// Effective config with the secret redacted (for `/v1/admin/config`).
    #[must_use]
    pub fn redacted_summary(&self) -> serde_json::Value {
        serde_json::json!({
            "bind_addr": self.bind_addr,
            "db_path": self.db_path.display().to_string(),
            "jwt_ttl_hours": self.jwt_ttl_hours,
            "tenant": self.tenant,
            "cors_origins": self.cors_origins,
            "rate_limit_rps": self.rate_limit_rps,
            "login_rate_per_min": self.login_rate_per_min,
            "body_limit_bytes": self.body_limit_bytes,
            "timeout_secs": self.timeout_secs,
            "i18n_extra_dirs": self.i18n_extra_dirs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "default_locale": self.default_locale.tag(),
            "webhook_timeout_secs": self.webhook_timeout_secs,
            "jwt_secret": "***redacted***",
        })
    }
}
