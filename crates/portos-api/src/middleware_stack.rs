//! Cross-cutting HTTP middleware: request ids, metrics, security headers,
//! rate limiting.

use axum::extract::Request;
use axum::extract::{ConnectInfo, MatchedPath, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;
use std::time::Instant;
use uuid::Uuid;

use crate::state::AppState;

/// Client IP for rate limiting (absent in unit tests → stable fallback).
/// Behind a reverse proxy the forwarding header is only trusted when the
/// deployment explicitly opts in (`PORTOS_TRUST_PROXY=1`), so untrusted
/// clients cannot spoof their identity.
fn client_ip(req: &Request, trust_proxy: bool) -> String {
    if trust_proxy {
        if let Some(ip) = req
            .headers()
            .get("x-forwarded-for")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            return ip.to_string();
        }
    }
    req.extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0.ip().to_string())
        .unwrap_or_else(|| "unix".into())
}

/// Propagate `X-Request-Id` and stamp it into logs and the response (§6.5).
pub async fn request_id(mut req: Request, next: Next) -> Response {
    let id = req
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.len() <= 128 && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let span = tracing::info_span!("http_request", request_id = %id);
    req.extensions_mut().insert(id.clone());
    let mut res = next.run(req).await;
    if let Ok(v) = HeaderValue::from_str(&id) {
        res.headers_mut()
            .insert(HeaderName::from_static("x-request-id"), v);
    }
    drop(span);
    res
}

/// HTTP duration + counter metrics in Prometheus exposition format.
pub async fn http_metrics(req: Request, next: Next) -> Response {
    let start = Instant::now();
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "unmatched".into());
    let method = req.method().as_str().to_string();
    let res = next.run(req).await;
    let elapsed = start.elapsed().as_secs_f64();
    let status = res.status().as_u16();
    crate::metrics::METRICS.inc(
        &format!(
            "portos_http_requests_total{{method=\"{method}\",path=\"{path}\",status=\"{status}\"}}"
        ),
        1,
    );
    crate::metrics::METRICS.observe("portos_http_request_duration_seconds", elapsed);
    res
}

/// Baseline security headers on every response (§6.3).
pub async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(
        HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        HeaderName::from_static("x-frame-options"),
        HeaderValue::from_static("DENY"),
    );
    headers.insert(
        HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        HeaderName::from_static("cache-control"),
        HeaderValue::from_static("no-store"),
    );
    res
}

/// Global per-IP token bucket; login gets a stricter dedicated bucket.
pub async fn rate_limit(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let ip = client_ip(&req, state.config.trust_proxy);
    let is_login = req.uri().path().starts_with("/v1/auth/login");
    let allowed = if is_login {
        state.login_limiter.allow(&format!("login:{ip}"))
    } else {
        state.global_limiter.allow(&ip)
    };
    if !allowed {
        crate::metrics::METRICS.inc_by_one("portos_rate_limited_total");
        let title = state.t(
            portos_i18n::Locale::En,
            "error.rate_limited",
            &portos_i18n::Args::new(),
        );
        return ApiProblem::rate_limited(title);
    }
    next.run(req).await
}

/// Request timeout with a carve-out for streaming endpoints: an SSE
/// connection is long-lived by design and must not be cut after
/// `PORTOS_TIMEOUT_SECS` (§6.2).
pub async fn timeout(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if req.uri().path().starts_with("/v1/events/stream") {
        return next.run(req).await;
    }
    let limit = std::time::Duration::from_secs(state.config.timeout_secs.max(1));
    match tokio::time::timeout(limit, next.run(req)).await {
        Ok(res) => res,
        Err(_) => ApiProblem::timeout(),
    }
}

/// Small local alias so middleware can render problem+json without the
/// full handler plumbing.
struct ApiProblem;

impl ApiProblem {
    fn timeout() -> Response {
        (
            StatusCode::GATEWAY_TIMEOUT,
            [(
                axum::http::header::CONTENT_TYPE,
                "application/problem+json; charset=utf-8",
            )],
            axum::Json(serde_json::json!({
                "type": "https://portos.dev/problems/timeout",
                "title": "request timed out",
                "status": 504,
            })),
        )
            .into_response()
    }

    fn rate_limited(title: String) -> Response {
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(
                axum::http::header::CONTENT_TYPE,
                "application/problem+json; charset=utf-8",
            )],
            axum::Json(serde_json::json!({
                "type": "https://portos.dev/problems/rate-limited",
                "title": title,
                "status": 429,
            })),
        )
            .into_response()
    }
}
