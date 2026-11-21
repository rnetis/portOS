//! Cross-cutting HTTP middleware: request ids, metrics, security headers,
//! rate limiting.

use axum::extract::{ConnectInfo, MatchedPath, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::extract::Request;
use axum::response::{IntoResponse, Response};
use std::net::SocketAddr;
use std::time::Instant;
use uuid::Uuid;

use crate::state::AppState;

/// Client IP for rate limiting (absent in unit tests → stable fallback).
fn client_ip(req: &Request) -> String {
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
        res.headers_mut().insert(HeaderName::from_static("x-request-id"), v);
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
        &format!("portos_http_requests_total{{method=\"{method}\",path=\"{path}\",status=\"{status}\"}}"),
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
pub async fn rate_limit(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let ip = client_ip(&req);
    let is_login = req.uri().path().starts_with("/v1/auth/login");
    let allowed = if is_login {
        state.login_limiter.allow(&format!("login:{ip}"))
    } else {
        state.global_limiter.allow(&ip)
    };
    if !allowed {
        crate::metrics::METRICS.inc_by_one("portos_rate_limited_total");
        let title = state.t(portos_i18n::Locale::En, "error.rate_limited", &portos_i18n::Args::new());
        return ApiProblem::rate_limited(title);
    }
    next.run(req).await
}

/// Small local alias so middleware can render problem+json without the
/// full handler plumbing.
struct ApiProblem;

impl ApiProblem {
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
