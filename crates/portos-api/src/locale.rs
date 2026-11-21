//! Locale resolution: query param → `Accept-Language` → saved user
//! preference → server default (PRD L-1).
//!
//! Display-only preferences ride along: calendar (`X-Calendar` /
//! `?calendar=`) and numerals (`X-Numerals` / `?numerals=`).

use axum::extract::State;
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::extract::Request;
use portos_i18n::{defaults_for, Calendar, Locale, Numerals};

use crate::state::AppState;

/// Resolved per-request display preferences.
#[derive(Debug, Clone, Copy)]
pub struct LocaleCtx {
    pub locale: Locale,
    pub calendar: Calendar,
    pub numerals: Numerals,
}

impl Default for LocaleCtx {
    fn default() -> Self {
        Self { locale: Locale::En, calendar: Calendar::Gregorian, numerals: Numerals::Western }
    }
}

impl LocaleCtx {
    /// Persian users get Jalali dates and Persian digits unless they
    /// explicitly chose otherwise (L-2/L-3/L-4).
    #[must_use]
    pub fn with_locale_defaults(locale: Locale) -> Self {
        let (calendar, numerals) = defaults_for(locale);
        Self { locale, calendar, numerals }
    }
}

fn qp(query: &Option<String>, key: &str) -> Option<String> {
    query.as_ref().and_then(|q| {
        q.split('&').find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k == key).then(|| v.to_string())
        })
    })
}

/// Build the context for a request.
#[must_use]
pub fn resolve(
    state: &AppState,
    headers: &HeaderMap,
    query: &Option<String>,
    user_locale: Option<Locale>,
    user_calendar: Option<Calendar>,
    user_numerals: Option<Numerals>,
) -> LocaleCtx {
    let mut ctx = LocaleCtx::with_locale_defaults(user_locale.unwrap_or(state.config.default_locale));

    // Language: explicit query param beats header beats user default.
    if let Some(lang) = qp(query, "lang").and_then(|v| Locale::parse(&v)) {
        ctx.locale = lang;
    } else if let Some(accept) = headers.get("accept-language").and_then(|h| h.to_str().ok()) {
        if let Some(l) = Locale::parse_accept_language(accept) {
            ctx.locale = l;
        }
    }

    ctx.calendar = user_calendar.unwrap_or(ctx.calendar);
    if let Some(cal) = qp(query, "calendar").and_then(|v| Calendar::parse(&v)) {
        ctx.calendar = cal;
    } else if let Some(h) = headers.get("x-calendar").and_then(|h| h.to_str().ok()) {
        if let Some(cal) = Calendar::parse(h) {
            ctx.calendar = cal;
        }
    }

    ctx.numerals = user_numerals.unwrap_or(ctx.numerals);
    if let Some(n) = qp(query, "numerals").and_then(|v| Numerals::parse(&v)) {
        ctx.numerals = n;
    } else if let Some(h) = headers.get("x-numerals").and_then(|h| h.to_str().ok()) {
        if let Some(n) = Numerals::parse(h) {
            ctx.numerals = n;
        }
    }
    ctx
}

/// Middleware: resolve display preferences and insert into extensions.
pub async fn locale_middleware(
    State(state): State<AppState>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
    headers: HeaderMap,
    mut req: Request,
    next: Next,
) -> axum::response::Response {
    // A saved user preference (authenticated) wins over the server default.
    let prefs = req.extensions().get::<crate::auth::AuthContext>().map(|a| (a.username.clone(), a.user_id.clone()));
    let (user_locale, user_calendar, user_numerals) = if let Some((username, _uid)) = prefs {
        match crate::db::db(&state.store, move |store| {
            Ok(portos_store::users::find_by_username(store, &username)?
                .map(|u| (Some(u.locale), Some(u.calendar), Some(u.numerals)))
                .unwrap_or((None, None, None)))
        })
        .await
        {
            Ok((l, c, n)) => (l, c, n),
            Err(_) => (None, None, None),
        }
    } else {
        (None, None, None)
    };

    let ctx = resolve(&state, &headers, &raw, user_locale, user_calendar, user_numerals);
    req.extensions_mut().insert(ctx);
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_locale() {
        let fa = LocaleCtx::with_locale_defaults(Locale::Fa);
        assert_eq!(fa.calendar, Calendar::Jalali);
        assert_eq!(fa.numerals, Numerals::Persian);
        let en = LocaleCtx::with_locale_defaults(Locale::En);
        assert_eq!(en.calendar, Calendar::Gregorian);
    }
}
