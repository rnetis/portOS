//! Localization endpoints (PRD L-10 resource files + L-3/L-4/L-9 demos).

use axum::extract::{Query, State};
use axum::Json;
use std::collections::HashMap;

use crate::error::ApiResult;
use crate::state::AppState;

/// GET /v1/i18n/catalog?locale=fa — the full resource bundle so web/mobile
/// frontends can render offline (L-10: languages updateable without a code
/// release via override directories).
pub async fn catalog(
    State(state): State<AppState>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let locale = q
        .get("locale")
        .and_then(|v| portos_i18n::Locale::parse(v))
        .unwrap_or(state.config.default_locale);
    let catalogs = state
        .catalogs
        .read()
        .map_err(|_| portos_core::CoreError::Internal("catalog lock".into()))?;
    let cat = catalogs.catalog(locale);
    let mut entries = serde_json::Map::new();
    for key in cat.keys() {
        entries.insert(
            key.to_string(),
            serde_json::Value::String(cat.t(key, &portos_i18n::Args::new())),
        );
    }
    Ok(Json(serde_json::json!({
        "locale": locale.tag(),
        "rtl": locale.rtl(),
        "default_calendar": portos_i18n::defaults_for(locale).0,
        "default_numerals": portos_i18n::defaults_for(locale).1,
        "entries": entries,
    })))
}

/// GET /v1/i18n/format-demo?date=2026-11-21&money=850000&currency=IRR
/// — shows the same data rendered per locale/calendar/numerals; useful for
/// localization QA (§3.2) and for clients without a formatting library.
pub async fn format_demo(
    State(state): State<AppState>,
    axum::Extension(ctx): axum::Extension<crate::locale::LocaleCtx>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let date = q
        .get("date")
        .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .unwrap_or_else(|| chrono::Utc::now().date_naive());
    let currency = q
        .get("currency")
        .and_then(|c| portos_core::money::Currency::parse(c).ok())
        .unwrap_or(portos_core::money::Currency::IRR);
    let minor = q
        .get("money")
        .and_then(|m| portos_i18n::numerals::parse_i64(m).ok())
        .unwrap_or(850_000);
    let money = portos_core::money::Money::new(minor, currency);

    let calendar = q
        .get("calendar")
        .and_then(|c| portos_i18n::Calendar::parse(c))
        .unwrap_or(ctx.calendar);
    let numerals = q
        .get("numerals")
        .and_then(|n| portos_i18n::Numerals::parse(n))
        .unwrap_or(ctx.numerals);

    Ok(Json(serde_json::json!({
        "locale": ctx.locale.tag(),
        "rtl": ctx.locale.rtl(),
        "date_gregorian_numeric": portos_i18n::formats::date_gregorian_numeric(date, numerals),
        "date_jalali_numeric": portos_i18n::formats::date_jalali_numeric(date, numerals),
        "date_long": portos_i18n::formats::date_long(ctx.locale, calendar, date, numerals),
        "jalali_long_fa": {
            "year": portos_i18n::JalaliDate::from_gregorian(date).year,
            "month_name_fa": portos_i18n::JalaliDate::from_gregorian(date).month_name_fa(),
            "month_name_en": portos_i18n::JalaliDate::from_gregorian(date).month_name_en(),
        },
        "money": portos_i18n::formats::money_display(ctx.locale, numerals, money),
        "toman": portos_i18n::formats::toman_display(ctx.locale, numerals, money),
        "bidi_wrapped_example": portos_i18n::text::wrap_ltr_runs(&format!(
            "{} {}",
            state.t(ctx.locale, "yard.container.placed", &portos_i18n::Args::new().arg("container", "MSKU3881107")),
            ""
        )),
    })))
}
