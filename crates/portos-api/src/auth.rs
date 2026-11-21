//! Authentication: Argon2id password hashing, JWT access tokens and
//! role→permission authorization (PRD F-44, §6.3).

use axum::extract::Request;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use portos_core::{CoreError, CoreResult};
use portos_i18n::Args;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

use crate::state::AppState;

/// Roles (F-44). Permission granularity is per capability; delegation and
/// custom roles come via the admin API in later phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Admin,
    OpsPlanner,
    YardPlanner,
    GateClerk,
    LineAgent,
    Customs,
    Finance,
    Viewer,
}

impl Role {
    pub fn parse(s: &str) -> CoreResult<Role> {
        match s {
            "admin" => Ok(Role::Admin),
            "ops_planner" => Ok(Role::OpsPlanner),
            "yard_planner" => Ok(Role::YardPlanner),
            "gate_clerk" => Ok(Role::GateClerk),
            "line_agent" => Ok(Role::LineAgent),
            "customs" => Ok(Role::Customs),
            "finance" => Ok(Role::Finance),
            "viewer" => Ok(Role::Viewer),
            other => Err(CoreError::Validation(format!("unknown role: {other}"))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::OpsPlanner => "ops_planner",
            Role::YardPlanner => "yard_planner",
            Role::GateClerk => "gate_clerk",
            Role::LineAgent => "line_agent",
            Role::Customs => "customs",
            Role::Finance => "finance",
            Role::Viewer => "viewer",
        }
    }

    /// The capability strings this role may use. `admin` holds everything.
    #[must_use]
    pub fn permissions(self) -> HashSet<&'static str> {
        use Role::*;
        let mut base: HashSet<&'static str> = HashSet::new();
        // Every authenticated role can view dashboards and subscribe.
        base.extend(["kpi.view", "event.subscribe"]);
        match self {
            Admin => base
                .union(&HashSet::from([
                    "vessel.manage",
                    "berth.manage",
                    "berth.allocate",
                    "yard.manage",
                    "gate.operate",
                    "gate.manage",
                    "billing.view",
                    "billing.manage",
                    "tariff.manage",
                    "edi.import",
                    "edi.export",
                    "work.dispatch",
                    "work.manage",
                    "incident.report",
                    "admin.manage",
                    "edi.process",
                ]))
                .copied()
                .collect(),
            OpsPlanner => base
                .union(&HashSet::from([
                    "vessel.manage",
                    "berth.manage",
                    "berth.allocate",
                    "work.dispatch",
                    "work.manage",
                    "gate.manage",
                    "billing.view",
                    "edi.import",
                    "edi.process",
                ]))
                .copied()
                .collect(),
            YardPlanner => base
                .union(&HashSet::from([
                    "yard.manage",
                    "work.dispatch",
                    "work.manage",
                    "gate.operate",
                ]))
                .copied()
                .collect(),
            GateClerk => base
                .union(&HashSet::from([
                    "gate.operate",
                    "yard.manage",
                    "incident.report",
                ]))
                .copied()
                .collect(),
            LineAgent => base
                .union(&HashSet::from([
                    "vessel.manage",
                    "billing.view",
                    "edi.import",
                    "edi.export",
                ]))
                .copied()
                .collect(),
            Customs => base
                .union(&HashSet::from([
                    "yard.manage",
                    "gate.operate",
                    "edi.import",
                    "edi.process",
                ]))
                .copied()
                .collect(),
            Finance => base
                .union(&HashSet::from([
                    "billing.view",
                    "billing.manage",
                    "tariff.manage",
                ]))
                .copied()
                .collect(),
            Viewer => base,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject: username.
    pub sub: String,
    /// User id.
    pub uid: String,
    pub role: String,
    pub exp: i64,
    pub iat: i64,
    pub jti: String,
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub user_id: String,
    pub username: String,
    pub role: String,
    pub permissions: HashSet<&'static str>,
    pub expires_at: i64,
}

impl AuthContext {
    /// Fail with Forbidden when the capability is missing (F-44).
    pub fn require(&self, permission: &str) -> CoreResult<()> {
        if self.permissions.contains(permission) {
            Ok(())
        } else {
            Err(CoreError::Forbidden)
        }
    }
}

/// Hash a password with Argon2id (recommended parameters).
pub fn hash_password(password: &str) -> CoreResult<String> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    use argon2::Argon2;
    use rand_core::OsRng;
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| CoreError::Internal(format!("password hash: {e}")))
}

/// Verify a password against a stored Argon2 PHC string.
pub fn verify_password(password: &str, hash: &str) -> CoreResult<bool> {
    use argon2::password_hash::PasswordVerifier;
    use argon2::Argon2;
    let parsed = argon2::password_hash::PasswordHash::new(hash)
        .map_err(|_| CoreError::Validation("stored hash is malformed".into()))?;
    let ok = Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok();
    Ok(ok)
}

/// Issue an access token.
pub fn issue_token(
    state: &AppState,
    username: &str,
    user_id: &str,
    role: &str,
) -> CoreResult<String> {
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: username.to_string(),
        uid: user_id.to_string(),
        role: role.to_string(),
        exp: now + state.config.jwt_ttl_hours * 3600,
        iat: now,
        jti: Uuid::now_v7().to_string(),
    };
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(state.config.jwt_secret.as_bytes()),
    )
    .map_err(|e| CoreError::Internal(format!("jwt: {e}")))
}

fn decode_token(state: &AppState, token: &str) -> CoreResult<Claims> {
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(state.config.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| CoreError::Unauthorized)?;
    Ok(data.claims)
}

/// Extract a bearer token from headers, or from `?token=` (SSE clients
/// cannot set headers).
pub fn bearer_token(headers: &HeaderMap, query_token: Option<&str>) -> Option<String> {
    if let Some(v) = headers.get("authorization").and_then(|h| h.to_str().ok()) {
        let v = v.trim();
        if let Some(rest) = v
            .strip_prefix("Bearer ")
            .or_else(|| v.strip_prefix("bearer "))
        {
            return Some(rest.trim().to_string());
        }
    }
    query_token
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
}

/// Authentication middleware: validates the JWT and inserts
/// [`AuthContext`]. Public routes are skipped by the router layout.
pub async fn auth_middleware(
    State(state): State<AppState>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    headers: HeaderMap,
    mut req: Request,
    next: Next,
) -> Response {
    let token = bearer_token(
        &headers,
        raw.as_deref()
            .and_then(|q| q.split('&').find_map(|pair| pair.strip_prefix("token="))),
    );
    let Some(token) = token else {
        return unauthorized(&state);
    };
    match decode_token(&state, &token) {
        Ok(claims) => {
            // Tokens are stateless, so re-check the account on every request:
            // deactivating a user must revoke access immediately.
            let Ok(uid) = claims.uid.parse::<uuid::Uuid>() else {
                return unauthorized(&state);
            };
            let uid = portos_core::ids::UserId::from_uuid(uid);
            let active = crate::db::db(&state.store, move |s| {
                portos_store::users::find_by_id(s, uid)
            })
            .await
            .ok()
            .flatten()
            .is_some_and(|u| u.active);
            if !active {
                return unauthorized(&state);
            }
            let role = Role::parse(&claims.role).unwrap_or(Role::Viewer);
            req.extensions_mut().insert(AuthContext {
                user_id: claims.uid,
                username: claims.sub,
                role: role.as_str().to_string(),
                permissions: role.permissions(),
                expires_at: claims.exp,
            });
            next.run(req).await
        }
        Err(_) => unauthorized(&state),
    }
}

fn unauthorized(state: &AppState) -> Response {
    let title = state.t(portos_i18n::Locale::En, "error.unauthorized", &Args::new());
    (
        axum::http::StatusCode::UNAUTHORIZED,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/problem+json; charset=utf-8",
        )],
        axum::Json(serde_json::json!({
            "type": "https://portos.dev/problems/unauthorized",
            "title": title,
            "status": 401,
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_permissions_cover_everything_for_admin() {
        for p in [
            "vessel.manage",
            "berth.allocate",
            "yard.manage",
            "gate.operate",
            "billing.manage",
            "tariff.manage",
            "edi.import",
            "work.dispatch",
            "admin.manage",
            "kpi.view",
        ] {
            assert!(Role::Admin.permissions().contains(p), "admin lacks {p}");
        }
        assert!(!Role::Viewer.permissions().contains("gate.operate"));
        assert!(Role::GateClerk.permissions().contains("gate.operate"));
        assert!(Role::Customs.permissions().contains("edi.process"));
    }

    #[test]
    fn argon2_round_trip() {
        let hash = hash_password("S3cret-تست!").unwrap();
        assert!(hash.starts_with("$argon2"));
        assert!(verify_password("S3cret-تست!", &hash).unwrap());
        assert!(!verify_password("wrong", &hash).unwrap());
        assert!(verify_password("x", "not-a-hash").is_err());
    }

    #[test]
    fn unknown_role_rejected() {
        assert!(Role::parse("superuser").is_err());
    }
}
