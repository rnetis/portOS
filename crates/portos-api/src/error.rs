//! API error model: RFC 7807 `application/problem+json` with localized
//! titles/details (PRD L-8) and stable machine-readable `type` URIs.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use portos_core::{CoreError, CoreResult};
use portos_i18n::{Args, CatalogSet, Locale};

/// A fully-rendered error response. Handlers build it via [`problem`]
/// (or [`problem_from`]) so localization happens where the state and
/// locale context are available.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    body: serde_json::Value,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            [("content-type", "application/problem+json; charset=utf-8")],
            Json(self.body),
        )
            .into_response()
    }
}

impl ApiError {
    pub fn raw(status: StatusCode, body: serde_json::Value) -> ApiError {
        ApiError { status, body }
    }
}

/// HTTP status mapping for core errors.
#[must_use]
pub fn status_for(err: &CoreError) -> StatusCode {
    match err {
        CoreError::NotFound(_) => StatusCode::NOT_FOUND,
        CoreError::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
        CoreError::Conflict(_) => StatusCode::CONFLICT,
        CoreError::Unauthorized => StatusCode::UNAUTHORIZED,
        CoreError::Forbidden => StatusCode::FORBIDDEN,
        CoreError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        CoreError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Build a localized problem+json response from a [`CoreError`].
pub fn problem_from(state: &crate::state::AppState, locale: Locale, err: CoreError) -> ApiError {
    let detail = err.to_string();
    let key = err.message_key();
    let args = Args::new().arg("detail", &detail);
    let title = state.t(locale, key, &args);
    let type_uri = format!("https://portos.dev/problems/{}", err.kind());
    ApiError {
        status: status_for(&err),
        body: serde_json::json!({
            "type": type_uri,
            "title": title,
            "status": status_for(&err).as_u16(),
            "detail": title,
            "detail_raw": detail,
            "instance": err.kind(),
        }),
    }
}

/// Same as [`problem_from`] but resolving locale from the request context.
pub fn problem(
    state: &crate::state::AppState,
    ctx: &crate::locale::LocaleCtx,
    err: CoreError,
) -> ApiError {
    problem_from(state, ctx.locale, err)
}

/// Convenience conversion for handlers that have no state handy
/// (returns an English problem — prefer [`problem`] inside handlers).
impl From<CoreError> for ApiError {
    fn from(err: CoreError) -> Self {
        ApiError::raw(
            status_for(&err),
            serde_json::json!({
                "type": format!("https://portos.dev/problems/{}", err.kind()),
                "title": err.to_string(),
                "status": status_for(&err).as_u16(),
            }),
        )
    }
}

/// Alias used by handlers.
pub type ApiResult<T> = Result<Json<T>, ApiError>;

/// Map a blocking DB result into the handler's error type with
/// localization.
pub fn db<T>(
    state: &crate::state::AppState,
    ctx: &crate::locale::LocaleCtx,
    res: CoreResult<T>,
) -> ApiResult<T> {
    res.map(Json).map_err(|e| problem(state, ctx, e))
}
