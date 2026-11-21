//! Login, session info and saved language preferences (PRD L-1, F-44).

use axum::extract::{Extension, State};
use axum::Json;
use portos_core::{CoreError, CoreResult};
use portos_i18n::Args;
use serde::Deserialize;

use crate::auth::{hash_password, verify_password, AuthContext, Role};
use crate::db::db;
use crate::error::{problem, ApiResult};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// POST /v1/auth/login — returns a JWT access token (§6.3 strong auth;
/// MFA for privileged users is enforced by the identity provider when
/// SSO is configured, I-5).
pub async fn login(
    State(state): State<AppState>,
    Extension(ctx): Extension<crate::locale::LocaleCtx>,
    Json(body): Json<LoginRequest>,
) -> ApiResult<serde_json::Value> {
    let username = body.username.trim().to_lowercase();
    let password = body.password;

    let user = db(&state.store, move |store| {
        portos_store::users::find_by_username(store, &username)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;

    let Some(user) = user else {
        // Burn the same Argon2 work as a real verification so response time
        // does not reveal whether the username exists (§6.3).
        let _ = tokio::task::spawn_blocking(move || {
            let _ = verify_password(&password, dummy_password_hash());
        })
        .await;
        return Err(problem(&state, &ctx, CoreError::Unauthorized));
    };
    if !user.active {
        let key = "auth.account_disabled";
        let title = state.t(ctx.locale, key, &Args::new());
        return Err(crate::error::ApiError::raw(
            axum::http::StatusCode::FORBIDDEN,
            serde_json::json!({
                "type": "https://portos.dev/problems/account-disabled",
                "title": title,
                "status": 403,
            }),
        ));
    }
    // Argon2 is deliberately CPU-expensive: keep it off the async runtime.
    let password_to_check = password.clone();
    let stored_hash = user.password_hash.clone();
    let ok = tokio::task::spawn_blocking(move || verify_password(&password_to_check, &stored_hash))
        .await
        .map_err(|e| {
            problem(
                &state,
                &ctx,
                CoreError::Internal(format!("password check task: {e}")),
            )
        })?
        .map_err(|e| problem(&state, &ctx, e))?;
    if !ok {
        return Err(problem(&state, &ctx, CoreError::Unauthorized));
    }
    let token = crate::auth::issue_token(&state, &user.username, &user.id.to_string(), &user.role)
        .map_err(|e| problem(&state, &ctx, e))?;
    let role = Role::parse(&user.role).map_err(|e| problem(&state, &ctx, e))?;

    // Audit the successful login (F-40).
    let mut audit = portos_core::audit::AuditRecord::system("auth.login", "user", &user.username);
    audit.actor = user.username.clone();
    let _ = db(&state.store, move |store| {
        portos_store::misc::append_audit(store, &audit)
    })
    .await;

    crate::metrics::METRICS.inc_by_one("portos_logins_total");
    let title = state.t(
        ctx.locale,
        "auth.login.success",
        &Args::new().arg("name", &user.display_name),
    );
    Ok(Json(serde_json::json!({
        "access_token": token,
        "token_type": "Bearer",
        "expires_in": state.config.jwt_ttl_hours * 3600,
        "message": title,
        "user": {
            "username": user.username,
            "display_name": user.display_name,
            "role": role.as_str(),
            "locale": user.locale.tag(),
            "calendar": user.calendar,
            "numerals": user.numerals,
        },
    })))
}

/// Stable hash used only to equalize login timing for unknown usernames.
fn dummy_password_hash() -> &'static str {
    static DUMMY: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| hash_password("not-a-real-password-000").expect("dummy hash"));
    DUMMY.as_str()
}

/// GET /v1/auth/me — current identity and effective permissions.
pub async fn me(Extension(auth): Extension<AuthContext>) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::json!({
        "username": auth.username,
        "role": auth.role,
        "permissions": auth.permissions.iter().copied().collect::<Vec<_>>(),
        "expires_at": auth.expires_at,
    })))
}

/// Utility used by the seeder to hash initial passwords.
pub fn hash_for_seed(password: &str) -> CoreResult<String> {
    hash_password(password)
}

#[derive(Deserialize)]
pub struct PreferencesRequest {
    pub locale: Option<String>,
    pub calendar: Option<String>,
    pub numerals: Option<String>,
}

/// POST /v1/i18n/preferences — persist display preferences for the signed-in
/// user; language switching never loses session state (PRD L-1).
pub async fn set_preferences(
    State(state): State<AppState>,
    Extension(ctx): Extension<crate::locale::LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<PreferencesRequest>,
) -> ApiResult<serde_json::Value> {
    let locale = body.locale.as_deref().and_then(portos_i18n::Locale::parse);
    let calendar = body
        .calendar
        .as_deref()
        .and_then(portos_i18n::Calendar::parse);
    let numerals = body
        .numerals
        .as_deref()
        .and_then(portos_i18n::Numerals::parse);
    if body.locale.is_some() && locale.is_none() {
        return Err(problem(
            &state,
            &ctx,
            CoreError::Validation("unknown locale".into()),
        ));
    }
    let user_id = auth
        .user_id
        .parse()
        .map(portos_core::ids::UserId::from_uuid)
        .map_err(|_| CoreError::Validation("bad subject".into()))?;
    db(&state.store, move |store| {
        portos_store::users::update_preferences(store, user_id, locale, calendar, numerals)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;

    let key = match ctx.locale {
        portos_i18n::Locale::Fa => "auth.language_switched",
        portos_i18n::Locale::En => "auth.language_switched",
    };
    Ok(Json(serde_json::json!({
        "message": state.t(ctx.locale, key, &Args::new()),
        "locale": locale.map(|l| l.tag()),
        "calendar": calendar,
        "numerals": numerals,
    })))
}
