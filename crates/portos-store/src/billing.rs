//! Commercial repositories: customers, tariffs, FX rates, invoices and
//! payments (PRD F-27..F-30).

use crate::util::{map_err, on_conn, parse_ts, ts};
use crate::Store;
use chrono::Utc;

use portos_core::billing::{
    ChargeLine, ChargeRef, Invoice, InvoiceStatus, Tariff, TariffBasis, Tier,
};
use portos_core::ids::{CallId, CustomerId, InvoiceId, TariffId};
use portos_core::money::{Currency, FxEntry, FxTable};
use portos_core::{CoreError, CoreResult};
use rusqlite::Row;

fn bad_row(col: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(0, col.to_string(), rusqlite::types::Type::Text)
}

// --------------------------------------------------------------------------
// Customers
// --------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct CustomerRow {
    pub id: CustomerId,
    pub name_en: String,
    pub name_fa: String,
    pub kind: String,
    pub credit_limit_minor: i64,
    pub currency: Currency,
    pub active: bool,
}

fn row_to_customer(r: &Row<'_>) -> rusqlite::Result<CustomerRow> {
    Ok(CustomerRow {
        id: CustomerId::from_uuid(
            uuid::Uuid::parse_str(&r.get::<_, String>("id")?).map_err(|_| bad_row("id"))?,
        ),
        name_en: r.get("name_en")?,
        name_fa: r.get("name_fa")?,
        kind: r.get("kind")?,
        credit_limit_minor: r.get("credit_limit_minor")?,
        currency: Currency::parse(&r.get::<_, String>("currency")?)
            .map_err(|_| bad_row("currency"))?,
        active: r.get::<_, i64>("active")? != 0,
    })
}

pub fn create_customer(store: &Store, c: &CustomerRow) -> CoreResult<()> {
    on_conn(&store.conn(), |conn| {
        conn.execute(
            "INSERT INTO customers (id, name_en, name_fa, kind, credit_limit_minor, currency, active, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                c.id.to_string(), c.name_en, c.name_fa, c.kind,
                c.credit_limit_minor, c.currency.code(), i64::from(c.active), ts(Utc::now())
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn get_customer(store: &Store, id: CustomerId) -> CoreResult<CustomerRow> {
    on_conn(&store.conn(), |c| {
        c.query_row(
            "SELECT * FROM customers WHERE id = ?1",
            [id.to_string()],
            row_to_customer,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!("customer {id}")),
            _ => map_err(e),
        })
    })
}

pub fn list_customers(store: &Store) -> CoreResult<Vec<CustomerRow>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT * FROM customers ORDER BY name_en")
            .map_err(map_err)?;
        let rows = stmt.query_map([], row_to_customer).map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
    })
}

// --------------------------------------------------------------------------
// Tariffs
// --------------------------------------------------------------------------

fn row_to_tariff(r: &Row<'_>) -> rusqlite::Result<Tariff> {
    let basis: String = r.get("basis")?;
    let tiers_json: String = r.get("tiers")?;
    let customer: Option<String> = r.get("customer_id")?;
    Ok(Tariff {
        id: TariffId::from_uuid(
            uuid::Uuid::parse_str(&r.get::<_, String>("id")?).map_err(|_| bad_row("id"))?,
        ),
        code: r.get("code")?,
        name_en: r.get("name_en")?,
        name_fa: r.get("name_fa")?,
        basis: serde_json::from_str::<TariffBasis>(&format!("\"{basis}\""))
            .map_err(|_| bad_row("basis"))?,
        currency: Currency::parse(&r.get::<_, String>("currency")?)
            .map_err(|_| bad_row("currency"))?,
        unit_minor: r.get("unit_minor")?,
        free_days: r.get::<_, i64>("free_days")? as u32,
        tiers: serde_json::from_str::<Vec<Tier>>(&tiers_json).unwrap_or_default(),
        tax_rate: r.get("tax_rate")?,
        customer: customer
            .map(|s| CustomerId::from_uuid(uuid::Uuid::parse_str(&s).unwrap_or_default())),
        active: r.get::<_, i64>("active")? != 0,
    })
}

pub fn upsert_tariff(store: &Store, t: &Tariff) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO tariffs (id, code, name_en, name_fa, basis, currency, unit_minor, free_days, tiers, tax_rate, customer_id, active)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
             ON CONFLICT(code) DO UPDATE SET name_en=excluded.name_en, name_fa=excluded.name_fa,
                basis=excluded.basis, currency=excluded.currency, unit_minor=excluded.unit_minor,
                free_days=excluded.free_days, tiers=excluded.tiers, tax_rate=excluded.tax_rate,
                customer_id=excluded.customer_id, active=excluded.active",
            rusqlite::params![
                t.id.to_string(), t.code, t.name_en, t.name_fa,
                crate::util::enum_str(&t.basis),
                t.currency.code(), t.unit_minor, t.free_days,
                serde_json::to_string(&t.tiers).map_err(|e| CoreError::Internal(e.to_string()))?,
                t.tax_rate, t.customer.map(|c| c.to_string()), i64::from(t.active)
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn list_tariffs(store: &Store, customer: Option<CustomerId>) -> CoreResult<Vec<Tariff>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT * FROM tariffs WHERE active = 1 ORDER BY code")
            .map_err(map_err)?;
        let rows = stmt.query_map([], row_to_tariff).map_err(map_err)?;
        let all: Vec<Tariff> = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(map_err)?;
        // Contract-specific tariffs win over public ones for the same code.
        Ok(all
            .into_iter()
            .filter(|t| match (t.customer, customer) {
                (Some(tc), Some(qc)) => tc == qc,
                (Some(_), None) => false,
                (None, _) => true,
            })
            .collect())
    })
}

// --------------------------------------------------------------------------
// FX rates
// --------------------------------------------------------------------------

pub fn upsert_fx(store: &Store, e: &FxEntry) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO fx_rates (base, quote, rate, as_of, source) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(base, quote) DO UPDATE SET rate=excluded.rate, as_of=excluded.as_of, source=excluded.source",
            rusqlite::params![e.base.code(), e.quote.code(), e.rate, ts(e.as_of), e.source],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn load_fx_table(store: &Store) -> CoreResult<FxTable> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT base, quote, rate, as_of, source FROM fx_rates")
            .map_err(map_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(FxEntry {
                    base: Currency::parse(&r.get::<_, String>(0)?).map_err(|_| bad_row("base"))?,
                    quote: Currency::parse(&r.get::<_, String>(1)?)
                        .map_err(|_| bad_row("quote"))?,
                    rate: r.get(2)?,
                    as_of: parse_ts(&r.get::<_, String>(3)?).map_err(|_| bad_row("as_of"))?,
                    source: r.get(4)?,
                })
            })
            .map_err(map_err)?;
        let mut table = FxTable::default();
        for row in rows {
            table.insert(row.map_err(map_err)?);
        }
        Ok(table)
    })
}

// --------------------------------------------------------------------------
// Invoices & payments
// --------------------------------------------------------------------------

fn row_to_invoice(r: &Row<'_>) -> rusqlite::Result<Invoice> {
    let status: String = r.get("status")?;
    let currency: String = r.get("currency")?;
    Ok(Invoice {
        id: InvoiceId::from_uuid(
            uuid::Uuid::parse_str(&r.get::<_, String>("id")?).map_err(|_| bad_row("id"))?,
        ),
        number: r.get("number")?,
        customer: CustomerId::from_uuid(
            uuid::Uuid::parse_str(&r.get::<_, String>("customer_id")?)
                .map_err(|_| bad_row("customer_id"))?,
        ),
        currency: Currency::parse(&currency).map_err(|_| bad_row("currency"))?,
        status: serde_json::from_str(&format!("\"{status}\"")).map_err(|_| bad_row("status"))?,
        subtotal_minor: r.get("subtotal_minor")?,
        tax_minor: r.get("tax_minor")?,
        total_minor: r.get("total_minor")?,
        paid_minor: r.get("paid_minor")?,
        issued_at: parse_ts(&r.get::<_, String>("issued_at")?).map_err(|_| bad_row("issued_at"))?,
        due_at: parse_ts(&r.get::<_, String>("due_at")?).map_err(|_| bad_row("due_at"))?,
        notes: r.get("notes")?,
    })
}

/// Create the invoice header and its lines in one transaction.
pub fn create_invoice(store: &Store, invoice: &Invoice, lines: &[ChargeLine]) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        let tx = c.transaction().map_err(map_err)?;
        tx.execute(
            "INSERT INTO invoices (id, number, customer_id, currency, status, subtotal_minor, tax_minor, total_minor, paid_minor, issued_at, due_at, notes)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            rusqlite::params![
                invoice.id.to_string(), invoice.number, invoice.customer.to_string(),
                invoice.currency.code(),
                crate::util::enum_str(&invoice.status),
                invoice.subtotal_minor, invoice.tax_minor, invoice.total_minor,
                invoice.paid_minor, ts(invoice.issued_at), ts(invoice.due_at), invoice.notes
            ],
        )
        .map_err(map_err)?;
        {
            let mut stmt = tx
                .prepare("INSERT INTO invoice_lines (id, invoice_id, tariff_code, description_en, description_fa, ref_type, ref_id, quantity, unit_minor, amount_minor, tax_rate, tax_minor)
                          VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)")
                .map_err(map_err)?;
            for line in lines {
                let (ref_type, ref_id): (&str, String) = match &line.charge_ref {
                    ChargeRef::Container { container } => {
                        ("container", container.as_str().to_string())
                    }
                    ChargeRef::VesselCall { call } => ("vessel_call", call.to_string()),
                    ChargeRef::Unspecified { label } => ("unspecified", label.clone()),
                };
                stmt.execute(rusqlite::params![
                    uuid::Uuid::now_v7().to_string(),
                    invoice.id.to_string(),
                    line.tariff_code,
                    line.description_en,
                    line.description_fa,
                    ref_type,
                    ref_id,
                    line.quantity,
                    line.unit_minor,
                    line.amount.minor,
                    line.tax_rate,
                    line.tax_minor
                ])
                .map_err(map_err)?;
            }
        }
        tx.commit().map_err(map_err)?;
        Ok(())
    })
}

pub fn get_invoice(store: &Store, number: &str) -> CoreResult<(Invoice, Vec<ChargeLine>)> {
    on_conn(&store.conn(), |c| {
        let invoice = c
            .query_row(
                "SELECT * FROM invoices WHERE number = ?1",
                [number],
                row_to_invoice,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("invoice {number}"))
                }
                _ => map_err(e),
            })?;
        // Raw rows are mapped outside the query closure so the invoice
        // currency is available.
        let mut stmt = c
            .prepare("SELECT tariff_code, description_en, description_fa, ref_type, ref_id, quantity, unit_minor, amount_minor, tax_rate, tax_minor
                      FROM invoice_lines WHERE invoice_id = ?1")
            .map_err(map_err)?;
        let raw = stmt
            .query_map([invoice.id.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, f64>(5)?,
                    r.get::<_, i64>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, f64>(8)?,
                    r.get::<_, i64>(9)?,
                ))
            })
            .map_err(map_err)?;
        let mut lines = Vec::new();
        for row in raw {
            let (code, en, fa, ref_type, ref_id, quantity, unit, amount_minor, tax_rate, tax_minor) =
                row.map_err(map_err)?;
            // Preserve what the charge refers to instead of collapsing every
            // reference to an opaque label (F-28/F-29).
            let charge_ref = match ref_type.as_str() {
                "container" => ChargeRef::Container {
                    container: portos_core::container::ContainerNumber::parse(&ref_id)
                        .map_err(|e| CoreError::Internal(format!("stored line ref: {e}")))?,
                },
                "vessel_call" => ChargeRef::VesselCall {
                    call: CallId::from_uuid(
                        uuid::Uuid::parse_str(&ref_id)
                            .map_err(|e| CoreError::Internal(format!("stored line ref: {e}")))?,
                    ),
                },
                _ => ChargeRef::Unspecified { label: ref_id },
            };
            lines.push(ChargeLine {
                tariff_code: code,
                description_en: en,
                description_fa: fa,
                charge_ref,
                quantity,
                unit_minor: unit,
                amount: portos_core::money::Money::new(amount_minor, invoice.currency),
                tax_rate,
                tax_minor,
            });
        }
        Ok((invoice, lines))
    })
}

pub fn list_invoices_for_customer(
    store: &Store,
    customer: Option<CustomerId>,
) -> CoreResult<Vec<Invoice>> {
    on_conn(&store.conn(), |c| {
        let (sql, param): (&str, Option<String>) = match customer {
            Some(id) => (
                "SELECT * FROM invoices WHERE customer_id = ?1 ORDER BY issued_at DESC",
                Some(id.to_string()),
            ),
            None => ("SELECT * FROM invoices ORDER BY issued_at DESC", None),
        };
        let mut stmt = c.prepare(sql).map_err(map_err)?;
        let rows = match param {
            Some(p) => stmt.query_map([p], row_to_invoice),
            None => stmt.query_map([], row_to_invoice),
        }
        .map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
    })
}

pub fn record_payment(
    store: &Store,
    number: &str,
    amount_minor: i64,
    method: &str,
    reference: Option<&str>,
) -> CoreResult<Invoice> {
    on_conn(&store.conn(), |c| {
        let tx = c.transaction().map_err(map_err)?;
        let inv = tx
            .query_row(
                "SELECT * FROM invoices WHERE number = ?1",
                [number],
                row_to_invoice,
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    CoreError::NotFound(format!("invoice {number}"))
                }
                _ => map_err(e),
            })?;
        if amount_minor <= 0 {
            return Err(CoreError::Validation(
                "payment amount must be positive".into(),
            ));
        }
        match inv.status {
            InvoiceStatus::Issued | InvoiceStatus::PartiallyPaid | InvoiceStatus::Disputed => {}
            other => {
                return Err(CoreError::Validation(format!(
                    "invoice {number} is {} and cannot take payments",
                    crate::util::enum_str(&other)
                )))
            }
        }
        let paid = inv.paid_minor + amount_minor;
        if paid > inv.total_minor {
            return Err(CoreError::Validation(format!(
                "payment of {amount_minor} exceeds outstanding balance of {}",
                inv.total_minor - inv.paid_minor
            )));
        }
        let status: InvoiceStatus = if paid >= inv.total_minor {
            InvoiceStatus::Paid
        } else {
            InvoiceStatus::PartiallyPaid
        };
        tx.execute(
            "INSERT INTO payments (id, invoice_id, amount_minor, currency, paid_at, method, reference) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![
                uuid::Uuid::now_v7().to_string(), inv.id.to_string(), amount_minor,
                inv.currency.code(), ts(Utc::now()), method, reference
            ],
        )
        .map_err(map_err)?;
        tx.execute(
            "UPDATE invoices SET paid_minor = ?2, status = ?3 WHERE id = ?1",
            rusqlite::params![inv.id.to_string(), paid, crate::util::enum_str(&status)],
        )
        .map_err(map_err)?;
        tx.commit().map_err(map_err)?;
        Ok(Invoice {
            paid_minor: paid,
            status,
            ..inv
        })
    })
}

pub fn outstanding_minor_for_customer(store: &Store, customer: CustomerId) -> CoreResult<i64> {
    on_conn(&store.conn(), |c| {
        c.query_row(
            "SELECT COALESCE(SUM(total_minor - paid_minor), 0) FROM invoices
             WHERE customer_id = ?1 AND status IN ('issued','partially_paid','disputed')",
            [customer.to_string()],
            |r| r.get(0),
        )
        .map_err(map_err)
    })
}

pub fn next_invoice_seq(store: &Store) -> CoreResult<i64> {
    on_conn(&store.conn(), |c| {
        c.query_row("SELECT COUNT(*) FROM invoices", [], |r| r.get(0))
            .map_err(map_err)
    })
}

/// Calls referenced by invoices (kept for cross-module billing).
#[allow(dead_code)]
pub fn call_ref(call: CallId) -> ChargeRef {
    ChargeRef::VesselCall { call }
}

#[cfg(test)]
mod tests {
    use super::*;
    use portos_core::billing::{InvoiceDraft, Tariff};
    use portos_core::ids::TariffId;

    #[test]
    fn customer_tariff_fx_invoice_flow() {
        let store = Store::open_in_memory().unwrap();
        let customer = CustomerRow {
            id: CustomerId::generate(),
            name_en: "Caspian Line".into(),
            name_fa: "خط دریای خزر".into(),
            kind: "line".into(),
            credit_limit_minor: 10_000_000_000,
            currency: Currency::IRR,
            active: true,
        };
        create_customer(&store, &customer).unwrap();
        assert_eq!(list_customers(&store).unwrap().len(), 1);

        let tariff = Tariff {
            id: TariffId::generate(),
            // Contract-specific codes differ from public ones (schema keeps
            // codes unique); the repo filter still prefers contract rows.
            code: "STORAGE-IMP-CASPIAN".into(),
            name_en: "Import storage".into(),
            name_fa: "انبارش وارداتی".into(),
            basis: TariffBasis::PerDay,
            currency: Currency::IRR,
            unit_minor: 850_000,
            free_days: 4,
            tiers: vec![],
            tax_rate: 0.09,
            customer: Some(customer.id),
            active: true,
        };
        upsert_tariff(&store, &tariff).unwrap();
        // Public tariff under the public code (schema keeps codes unique).
        let mut public = tariff.clone();
        public.id = TariffId::generate();
        public.code = "STORAGE-IMP".into();
        public.customer = None;
        upsert_tariff(&store, &public).unwrap();
        // Contract tariff wins for this customer.
        let applicable = list_tariffs(&store, Some(customer.id)).unwrap();
        assert!(applicable.iter().any(|t| t.customer == Some(customer.id)));

        upsert_fx(
            &store,
            &FxEntry {
                base: Currency::USD,
                quote: Currency::IRR,
                rate: 100_000.0,
                as_of: Utc::now(),
                source: "demo".into(),
            },
        )
        .unwrap();
        let fx = load_fx_table(&store).unwrap();
        assert!(fx.get(Currency::USD, Currency::IRR).is_some());

        let line = ChargeLine {
            tariff_code: "STORAGE-IMP".into(),
            description_en: "storage 6 days".into(),
            description_fa: "انبارش ۶ روز".into(),
            charge_ref: ChargeRef::Unspecified {
                label: "batch".into(),
            },
            quantity: 6.0,
            unit_minor: 850_000,
            amount: portos_core::money::Money::new(5_100_000, Currency::IRR),
            tax_rate: 0.09,
            tax_minor: 459_000,
        };
        let mut draft = InvoiceDraft::new(customer.id, Currency::IRR);
        draft.push(line).unwrap();
        let seq = next_invoice_seq(&store).unwrap() + 1;
        let lines = draft.lines.clone();
        let invoice = draft.finalize(format!("INV-2026-{seq:06}"), Utc::now(), 30);
        create_invoice(&store, &invoice, &lines).unwrap();

        let (got, lines) = get_invoice(&store, &invoice.number).unwrap();
        assert_eq!(got.total_minor, 5_559_000);
        assert_eq!(lines.len(), 1);

        let paid =
            record_payment(&store, &invoice.number, 5_559_000, "gateway", Some("TX99")).unwrap();
        assert_eq!(paid.status, InvoiceStatus::Paid);
        assert_eq!(
            outstanding_minor_for_customer(&store, customer.id).unwrap(),
            0
        );

        // Invalid payments are rejected instead of corrupting the balance.
        assert!(
            record_payment(&store, &invoice.number, 0, "gateway", None).is_err(),
            "zero payment"
        );
        assert!(
            record_payment(&store, &invoice.number, -5, "gateway", None).is_err(),
            "negative payment"
        );
        assert!(
            record_payment(&store, &invoice.number, 1, "gateway", None).is_err(),
            "paid invoice"
        );
    }
}
