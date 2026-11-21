//! Billing endpoints: customers, tariffs, FX, invoicing, payments
//! (PRD F-27..F-30).

use axum::extract::{Path, Query, State};
use axum::Extension;
use axum::Json;
use portos_core::billing::{self, ChargeRef, InvoiceDraft, Tariff, TariffBasis, Tier};
use portos_core::container::ContainerNumber;
use portos_core::ids::CustomerId;
use portos_core::money::{Currency, FxEntry, Money};
use portos_core::{CoreError, CoreResult};
use portos_i18n::Args;
use serde::Deserialize;
use std::collections::HashMap;

use crate::auth::AuthContext;
use crate::db::db;
use crate::error::{problem, ApiResult};
use crate::locale::LocaleCtx;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CustomerBody {
    pub name_en: String,
    pub name_fa: String,
    pub kind: String,
    pub credit_limit_minor: Option<i64>,
    pub currency: Option<String>,
}

/// POST /v1/billing/customers
pub async fn create_customer(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<CustomerBody>,
) -> ApiResult<serde_json::Value> {
    auth.require("billing.manage")
        .map_err(|e| problem(&state, &ctx, e))?;
    let currency = match &body.currency {
        Some(c) => Currency::parse(c).map_err(|e| problem(&state, &ctx, e))?,
        None => Currency::IRR,
    };
    let customer = portos_store::billing::CustomerRow {
        id: CustomerId::generate(),
        name_en: body.name_en,
        name_fa: body.name_fa,
        kind: body.kind,
        credit_limit_minor: body.credit_limit_minor.unwrap_or(0),
        currency,
        active: true,
    };
    let c = customer.clone();
    db(&state.store, move |store| {
        portos_store::billing::create_customer(store, &c)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(
        &state,
        &auth,
        "billing.customer.create",
        "customer",
        &customer.id.to_string(),
    )
    .await;
    Ok(Json(serde_json::to_value(&customer).unwrap_or_default()))
}

/// GET /v1/billing/customers
pub async fn list_customers(
    State(state): State<AppState>,
    Extension(_ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
) -> ApiResult<serde_json::Value> {
    let customers = db(&state.store, |store| {
        portos_store::billing::list_customers(store)
    })
    .await
    .map_err(|e| problem(&state, &_ctx, e))?;
    Ok(Json(serde_json::json!({ "customers": customers })))
}

#[derive(Deserialize)]
pub struct TariffBody {
    pub code: String,
    pub name_en: String,
    pub name_fa: String,
    pub basis: String,
    pub currency: String,
    pub unit_minor: i64,
    pub free_days: Option<u32>,
    #[serde(default)]
    pub tiers: Vec<Tier>,
    pub tax_rate: Option<f64>,
    pub customer_id: Option<String>,
}

/// POST /v1/billing/tariffs — configurable tariff engine entries (F-27).
pub async fn upsert_tariff(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<TariffBody>,
) -> ApiResult<serde_json::Value> {
    auth.require("tariff.manage")
        .map_err(|e| problem(&state, &ctx, e))?;
    let basis: TariffBasis = serde_json::from_str(&format!("\"{}\"", body.basis))
        .map_err(|_| CoreError::Validation(format!("unknown basis {}", body.basis)))
        .map_err(|e| problem(&state, &ctx, e))?;
    let currency = Currency::parse(&body.currency).map_err(|e| problem(&state, &ctx, e))?;
    let customer = body
        .customer_id
        .map(|s| {
            s.parse::<uuid::Uuid>()
                .map(CustomerId::from_uuid)
                .map_err(|_| CoreError::Validation("bad customer_id".into()))
        })
        .transpose()
        .map_err(|e| problem(&state, &ctx, e))?;
    let tariff = Tariff {
        id: portos_core::ids::TariffId::generate(),
        code: body.code,
        name_en: body.name_en,
        name_fa: body.name_fa,
        basis,
        currency,
        unit_minor: body.unit_minor,
        free_days: body.free_days.unwrap_or(0),
        tiers: body.tiers,
        tax_rate: body.tax_rate.unwrap_or(0.09),
        customer,
        active: true,
    };
    let t = tariff.clone();
    db(&state.store, move |store| {
        portos_store::billing::upsert_tariff(store, &t)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(
        &state,
        &auth,
        "billing.tariff.upsert",
        "tariff",
        &tariff.code,
    )
    .await;
    Ok(Json(serde_json::to_value(&tariff).unwrap_or_default()))
}

/// GET /v1/billing/tariffs?customer=
pub async fn list_tariffs(
    State(state): State<AppState>,
    Extension(_ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let customer = q
        .get("customer")
        .and_then(|s| s.parse::<uuid::Uuid>().ok())
        .map(CustomerId::from_uuid);
    let tariffs = db(&state.store, move |store| {
        portos_store::billing::list_tariffs(store, customer)
    })
    .await
    .map_err(|e| problem(&state, &_ctx, e))?;
    Ok(Json(serde_json::json!({ "tariffs": tariffs })))
}

#[derive(Deserialize)]
pub struct FxBody {
    pub base: String,
    pub quote: String,
    pub rate: f64,
}

/// POST /v1/billing/fx — exchange-rate management (L-9, I-3).
pub async fn set_fx(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<FxBody>,
) -> ApiResult<serde_json::Value> {
    auth.require("tariff.manage")
        .map_err(|e| problem(&state, &ctx, e))?;
    if body.rate <= 0.0 {
        return Err(problem(
            &state,
            &ctx,
            CoreError::Validation("rate must be positive".into()),
        ));
    }
    let entry = FxEntry {
        base: Currency::parse(&body.base).map_err(|e| problem(&state, &ctx, e))?,
        quote: Currency::parse(&body.quote).map_err(|e| problem(&state, &ctx, e))?,
        rate: body.rate,
        as_of: chrono::Utc::now(),
        source: "admin".into(),
    };
    let e = entry.clone();
    db(&state.store, move |store| {
        portos_store::billing::upsert_fx(store, &e)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::audit(
        &state,
        &auth,
        "billing.fx.update",
        "fx_rate",
        &format!("{}/{}", body.base, body.quote),
    )
    .await;
    let msg = state.t(
        ctx.locale,
        "admin.fx.updated",
        &Args::new()
            .arg("base", body.base.to_uppercase())
            .arg("quote", body.quote.to_uppercase())
            .arg("rate", format!("{}", body.rate)),
    );
    Ok(Json(serde_json::json!({ "fx": entry, "message": msg })))
}

#[derive(Deserialize)]
pub struct GenerateInvoice {
    pub customer_id: String,
    /// Storage/demurrage/reefer-plug charges for one container.
    pub container: Option<String>,
    /// Per-move + call fee charges for one vessel call.
    pub vessel_call_id: Option<String>,
    /// Currency of the invoice (defaults to the customer tariff currency).
    pub currency: Option<String>,
    /// Include disputed/draft status controls.
    pub notes: Option<String>,
}

/// POST /v1/billing/invoices/generate — automatic charge calculation and
/// invoicing (F-28, F-29). Container mode: storage/demurrage/reefer-plug
/// day charges. Call mode: handling moves + vessel call fee.
pub async fn generate_invoice(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Json(body): Json<GenerateInvoice>,
) -> ApiResult<serde_json::Value> {
    auth.require("billing.manage")
        .map_err(|e| problem(&state, &ctx, e))?;
    let customer_id = body
        .customer_id
        .parse::<uuid::Uuid>()
        .map(CustomerId::from_uuid)
        .map_err(|_| CoreError::Validation("bad customer_id".into()))
        .map_err(|e| problem(&state, &ctx, e))?;
    let now = chrono::Utc::now();
    let customer = db(&state.store, move |store| {
        portos_store::billing::get_customer(store, customer_id)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    let currency = match &body.currency {
        Some(c) => Currency::parse(c).map_err(|e| problem(&state, &ctx, e))?,
        None => customer.currency,
    };
    let tariffs = db(&state.store, move |store| {
        portos_store::billing::list_tariffs(store, Some(customer_id))
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    let fx = db(&state.store, |store| {
        portos_store::billing::load_fx_table(store)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;

    let mut draft = InvoiceDraft::new(customer_id, currency);

    if let Some(container_text) = &body.container {
        let cn = ContainerNumber::parse(container_text).map_err(|e| problem(&state, &ctx, e))?;
        let cn_lookup = cn.clone();
        let container = db(&state.store, move |store| {
            portos_store::yard::get_container(store, &cn_lookup)
        })
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
        let gate_in = gate_in_time(cn.clone(), &state).await?;
        // Storage (per-day) — free days from the tariff.
        let storage = find_tariff(&tariffs, "STORAGE-IMP")
            .ok_or_else(|| CoreError::Validation("tariff STORAGE-IMP not configured".into()))
            .map_err(|e| problem(&state, &ctx, e))?;
        let charge = billing::compute_day_charge(
            &convert_tariff(storage, currency, &fx)?,
            gate_in,
            None,
            now,
        )
        .map_err(|e| problem(&state, &ctx, e))?;
        draft.push(billing_line(
            storage,
            "billing.storage.line",
            &[
                ("days", charge.days_billed.to_string()),
                ("free", charge.free_days.to_string()),
            ],
            ChargeRef::Container {
                container: cn.clone(),
            },
            charge.days_billed as f64,
            charge.amount,
            currency,
            &fx,
        )?)?;

        // Reefer plug days (F-10/F-28) when the box is a reefer.
        if container.kind().needs_plug() {
            if let Some(plug) = find_tariff(&tariffs, "REEFER-PLUG") {
                let plug_converted = convert_tariff(plug, currency, &fx)?;
                let charge = billing::compute_day_charge(&plug_converted, gate_in, None, now)
                    .map_err(|e| problem(&state, &ctx, e))?;
                draft.push(billing_line(
                    plug,
                    "billing.reefer_plug.line",
                    &[("days", charge.days_billed.to_string())],
                    ChargeRef::Container { container: cn },
                    charge.days_billed as f64,
                    charge.amount,
                    currency,
                    &fx,
                )?)?;
            }
        }
    }

    if let Some(call_text) = &body.vessel_call_id {
        let cid = call_text
            .parse::<uuid::Uuid>()
            .map(portos_core::ids::CallId::from_uuid)
            .map_err(|_| CoreError::Validation("bad vessel_call_id".into()))
            .map_err(|e| problem(&state, &ctx, e))?;
        let call_moves = db(&state.store, move |store| {
            portos_store::work::count_moves_for_call(store, cid)
        })
        .await
        .map_err(|e| problem(&state, &ctx, e))?;
        if let Some(move_tariff) = find_tariff(&tariffs, "MOVE-DIS") {
            let line = billing::compute_move_charge(
                &convert_tariff(move_tariff, currency, &fx)?,
                call_moves,
                ChargeRef::VesselCall { call: cid },
            )
            .map_err(|e| problem(&state, &ctx, e))?;
            // Keep the tariff's original name/description for display.
            let mut line = line;
            line.tariff_code = move_tariff.code.clone();
            draft.push(line).map_err(|e| problem(&state, &ctx, e))?;
        }
        if let Some(call_fee) = find_tariff(&tariffs, "CALL-FEE") {
            let fee = convert_tariff(call_fee, currency, &fx)?;
            let amount = Money::new(fee.unit_minor, currency);
            draft
                .push(billing::ChargeLine {
                    tariff_code: call_fee.code.clone(),
                    description_en: call_fee.name_en.clone(),
                    description_fa: call_fee.name_fa.clone(),
                    charge_ref: ChargeRef::VesselCall { call: cid },
                    quantity: 1.0,
                    unit_minor: fee.unit_minor,
                    amount,
                    tax_rate: fee.tax_rate,
                    tax_minor: (amount.minor as f64 * fee.tax_rate).round() as i64,
                })
                .map_err(|e| problem(&state, &ctx, e))?;
        }
    }

    if draft.lines.is_empty() {
        return Err(problem(
            &state,
            &ctx,
            CoreError::Validation("nothing to invoice: provide container or vessel_call_id".into()),
        ));
    }
    // Within the free period every computed line is zero — issuing a zero
    // invoice would be noise in receivables, so report it explicitly.
    if draft.lines.iter().all(|l| l.amount.minor == 0) {
        return Err(problem(
            &state,
            &ctx,
            CoreError::Validation(
                "nothing to charge: all computed charges are zero (inside the free period)".into(),
            ),
        ));
    }

    let seq = db(&state.store, |store| {
        portos_store::billing::next_invoice_seq(store)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    let number = format!("INV-{}-{:06}", now.format("%Y"), seq + 1);
    let lines = draft.lines.clone();
    let lines_for_store = lines.clone();
    let invoice = draft.finalize(number.clone(), now, 30);
    let inv = invoice.clone();
    db(&state.store, move |store| {
        portos_store::billing::create_invoice(store, &inv, &lines_for_store)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::events::publish(
        &state,
        portos_core::events::DomainEventKind::InvoiceIssued {
            invoice: invoice.id,
            number: number.clone(),
            customer: customer_id,
            total_minor: invoice.total_minor,
            currency: currency.code().to_string(),
        },
    );
    crate::audit(&state, &auth, "billing.invoice.issue", "invoice", &number).await;
    let total_display = portos_i18n::formats::money_display(
        ctx.locale,
        ctx.numerals,
        Money::new(invoice.total_minor, currency),
    );
    let msg = state.t(
        ctx.locale,
        "billing.invoice.issued",
        &Args::new()
            .arg("number", &number)
            .arg("total", &total_display),
    );
    Ok(Json(
        serde_json::json!({ "invoice": invoice, "lines": lines, "message": msg }),
    ))
}

/// Gate-in timestamp of a container — storage starts at gate-in (F-28).
async fn gate_in_time(
    cn: ContainerNumber,
    state: &AppState,
) -> CoreResult<chrono::DateTime<chrono::Utc>> {
    let lookup = cn.clone();
    db(&state.store, move |store| {
        portos_store::yard::gate_in_at(store, &lookup)
    })
    .await?
    .ok_or_else(|| {
        CoreError::Validation(format!(
            "container {cn} has no gate-in time; storage starts at gate-in"
        ))
    })
}

fn find_tariff<'a>(tariffs: &'a [Tariff], code: &str) -> Option<&'a Tariff> {
    tariffs.iter().find(|t| t.code == code)
}

/// Convert a tariff into the invoice currency via the FX table (L-9).
fn convert_tariff(
    t: &Tariff,
    currency: Currency,
    fx: &portos_core::money::FxTable,
) -> CoreResult<Tariff> {
    if t.currency == currency {
        return Ok(t.clone());
    }
    let converted = fx.convert(Money::new(t.unit_minor, t.currency), currency)?;
    Ok(Tariff {
        unit_minor: converted.minor,
        currency,
        ..t.clone()
    })
}

/// Build a localized charge line from a day charge (bilingual line text,
/// L-8) and convert the amount into the invoice currency.
#[allow(clippy::too_many_arguments)]
fn billing_line(
    tariff: &Tariff,
    key: &str,
    args: &[(&str, String)],
    charge_ref: ChargeRef,
    quantity: f64,
    amount: Money,
    invoice_currency: Currency,
    fx: &portos_core::money::FxTable,
) -> CoreResult<billing::ChargeLine> {
    let amount = if amount.currency == invoice_currency {
        amount
    } else {
        fx.convert(amount, invoice_currency)?
    };
    let mut a = portos_i18n::Args::new();
    for (k, v) in args {
        a = a.arg(k, v);
    }
    Ok(billing::ChargeLine {
        tariff_code: tariff.code.clone(),
        description_en: format_args_en(key, args),
        description_fa: format_args_fa(key, args),
        charge_ref,
        quantity,
        unit_minor: if quantity > 0.0 {
            (amount.minor as f64 / quantity).round() as i64
        } else {
            amount.minor
        },
        amount,
        tax_rate: tariff.tax_rate,
        tax_minor: (amount.minor as f64 * tariff.tax_rate).round() as i64,
    })
}

fn format_args_en(key: &str, args: &[(&str, String)]) -> String {
    let templates = HashMap::from([
        (
            "billing.storage.line",
            "Yard storage ({days} day(s), {free} free)",
        ),
        (
            "billing.demurrage.line",
            "Demurrage — container {container} ({days} day(s))",
        ),
        (
            "billing.reefer_plug.line",
            "Reefer plug service ({days} day(s))",
        ),
    ]);
    interpolate(templates.get(key).copied().unwrap_or(key), args)
}

fn format_args_fa(key: &str, args: &[(&str, String)]) -> String {
    let templates = HashMap::from([
        (
            "billing.storage.line",
            "انبارش محوطه ({days} روز، {free} روز رایگان)",
        ),
        (
            "billing.demurrage.line",
            "دِمِرِج — کانتینر {container} ({days} روز)",
        ),
        ("billing.reefer_plug.line", "خدمات پریز یخچالی ({days} روز)"),
    ]);
    interpolate(templates.get(key).copied().unwrap_or(key), args)
}

fn interpolate(template: &str, args: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (k, v) in args {
        out = out.replace(&format!("{{{k}}}"), v);
    }
    out
}

/// GET /v1/billing/invoices/{number} — header + bilingual lines, with
/// localized date/amount display fields (L-8).
pub async fn get_invoice(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Path(number): Path<String>,
) -> ApiResult<serde_json::Value> {
    let num = number.clone();
    let (invoice, lines) = db(&state.store, move |store| {
        portos_store::billing::get_invoice(store, &num)
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    Ok(Json(serde_json::json!({
        "invoice": invoice,
        "lines": lines,
        "display": {
            "issued_at": portos_i18n::formats::datetime_display(ctx.locale, ctx.calendar, ctx.numerals, invoice.issued_at),
            "due_at": portos_i18n::formats::datetime_display(ctx.locale, ctx.calendar, ctx.numerals, invoice.due_at),
            "total": portos_i18n::formats::money_display(ctx.locale, ctx.numerals, Money::new(invoice.total_minor, invoice.currency)),
            "toman": portos_i18n::formats::toman_display(ctx.locale, ctx.numerals, Money::new(invoice.total_minor, invoice.currency)),
        },
    })))
}

/// GET /v1/billing/invoices?customer=
pub async fn list_invoices(
    State(state): State<AppState>,
    Extension(_ctx): Extension<LocaleCtx>,
    Extension(_auth): Extension<AuthContext>,
    Query(q): Query<HashMap<String, String>>,
) -> ApiResult<serde_json::Value> {
    let customer = q
        .get("customer")
        .and_then(|s| s.parse::<uuid::Uuid>().ok())
        .map(CustomerId::from_uuid);
    let invoices = db(&state.store, move |store| {
        portos_store::billing::list_invoices_for_customer(store, customer)
    })
    .await
    .map_err(|e| problem(&state, &_ctx, e))?;
    Ok(Json(serde_json::json!({ "invoices": invoices })))
}

#[derive(Deserialize)]
pub struct PaymentBody {
    pub amount_minor: i64,
    pub method: String,
    pub reference: Option<String>,
}

/// POST /v1/billing/invoices/{number}/payments — payment gateway
/// integration endpoint (F-29).
pub async fn record_payment(
    State(state): State<AppState>,
    Extension(ctx): Extension<LocaleCtx>,
    Extension(auth): Extension<AuthContext>,
    Path(number): Path<String>,
    Json(body): Json<PaymentBody>,
) -> ApiResult<serde_json::Value> {
    auth.require("billing.manage")
        .map_err(|e| problem(&state, &ctx, e))?;
    if body.amount_minor <= 0 {
        return Err(problem(
            &state,
            &ctx,
            CoreError::Validation("amount must be positive".into()),
        ));
    }
    let num = number.clone();
    let paid = db(&state.store, move |store| {
        portos_store::billing::record_payment(
            store,
            &num,
            body.amount_minor,
            &body.method,
            body.reference.as_deref(),
        )
    })
    .await
    .map_err(|e| problem(&state, &ctx, e))?;
    crate::events::publish(
        &state,
        portos_core::events::DomainEventKind::PaymentRecorded {
            number: number.clone(),
            amount_minor: body.amount_minor,
        },
    );
    crate::audit(&state, &auth, "billing.payment", "invoice", &number).await;
    let amount_display = portos_i18n::formats::money_display(
        ctx.locale,
        ctx.numerals,
        Money::new(body.amount_minor, paid.currency),
    );
    let msg = state.t(
        ctx.locale,
        "billing.invoice.paid",
        &Args::new()
            .arg("amount", &amount_display)
            .arg("number", &number),
    );
    Ok(Json(serde_json::json!({ "invoice": paid, "message": msg })))
}
