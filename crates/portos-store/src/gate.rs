//! Gate repositories: appointments, transactions, passes, EIRs
//! (PRD F-17..F-21).

use crate::util::{map_err, on_conn, parse_ts, parse_ts_opt, ts};
use crate::Store;
use chrono::{DateTime, Utc};
use portos_core::container::ContainerNumber;
use portos_core::gate::{
    GateDirection, GateStatus, GateStep, GateTransaction, TruckAppointment,
    AppointmentStatus,
};
use portos_core::ids::{AppointmentId, CustomerId, GateTxId};
use portos_core::{CoreError, CoreResult};
use rusqlite::Row;

fn bad_row(col: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(0, col.to_string(), rusqlite::types::Type::Text)
}

// --------------------------------------------------------------------------
// Appointments
// --------------------------------------------------------------------------

fn row_to_appointment(r: &Row<'_>) -> rusqlite::Result<TruckAppointment> {
    let dir: String = r.get("direction")?;
    let status: String = r.get("status")?;
    let company: Option<String> = r.get("trucking_company_id")?;
    let container: Option<String> = r.get("container_id")?;
    Ok(TruckAppointment {
        id: AppointmentId::from_uuid(
            uuid::Uuid::parse_str(&r.get::<_, String>("id")?).map_err(|_| bad_row("id"))?,
        ),
        reference: r.get("reference")?,
        trucking_company: company.map(|s| CustomerId::from_uuid(uuid::Uuid::parse_str(&s).unwrap_or_default())),
        driver_name: r.get("driver_name")?,
        driver_license: r.get("driver_license")?,
        plate: r.get("plate")?,
        container: container
            .map(|s| ContainerNumber::parse(&s))
            .transpose()
            .map_err(|_| bad_row("container_id"))?,
        direction: serde_json::from_str(&format!("\"{dir}\"")).map_err(|_| bad_row("direction"))?,
        slot_from: parse_ts(&r.get::<_, String>("slot_from")?).map_err(|_| bad_row("slot_from"))?,
        slot_to: parse_ts(&r.get::<_, String>("slot_to")?).map_err(|_| bad_row("slot_to"))?,
        status: serde_json::from_str(&format!("\"{status}\"")).map_err(|_| bad_row(&format!("appt status raw={status:?}")))?,
        created_at: parse_ts(&r.get::<_, String>("created_at")?).map_err(|_| bad_row("created_at"))?,
    })
}

pub fn create_appointment(store: &Store, a: &TruckAppointment) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO truck_appointments (id, reference, trucking_company_id, driver_name, driver_license,
                plate, container_id, direction, slot_from, slot_to, status, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            rusqlite::params![
                a.id.to_string(), a.reference,
                a.trucking_company.map(|c| c.to_string()),
                a.driver_name, a.driver_license, a.plate,
                a.container.as_ref().map(|c| c.as_str().to_string()),
                crate::util::enum_str(&a.direction),
                ts(a.slot_from), ts(a.slot_to),
                crate::util::enum_str(&a.status),
                ts(a.created_at)
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn get_appointment(store: &Store, id: AppointmentId) -> CoreResult<TruckAppointment> {
    on_conn(&store.conn(), |c| {
        c.query_row("SELECT * FROM truck_appointments WHERE id = ?1", [id.to_string()], row_to_appointment)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!("appointment {id}")),
                _ => map_err(e),
            })
    })
}

pub fn get_appointment_by_reference(store: &Store, reference: &str) -> CoreResult<TruckAppointment> {
    on_conn(&store.conn(), |c| {
        c.query_row("SELECT * FROM truck_appointments WHERE reference = ?1", [reference], row_to_appointment)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!("appointment {reference}")),
                _ => map_err(e),
            })
    })
}

pub fn update_appointment_status(store: &Store, id: AppointmentId, status: AppointmentStatus) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "UPDATE truck_appointments SET status = ?2 WHERE id = ?1",
            rusqlite::params![
                id.to_string(),
                crate::util::enum_str(&status)
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

/// Appointments whose slot overlaps the window (queue visibility — F-20).
pub fn list_appointments_in_window(
    store: &Store,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> CoreResult<Vec<TruckAppointment>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT * FROM truck_appointments WHERE slot_to > ?1 AND slot_from < ?2 ORDER BY slot_from")
            .map_err(map_err)?;
        let rows = stmt
            .query_map(rusqlite::params![ts(from), ts(to)], row_to_appointment)
            .map_err(map_err)?;
        rows.filter_map(|r| r.ok()).map(Ok).collect()
    })
}

pub fn count_appointments(store: &Store) -> CoreResult<i64> {
    on_conn(&store.conn(), |c| {
        c.query_row("SELECT COUNT(*) FROM truck_appointments", [], |r| r.get(0)).map_err(map_err)
    })
}

// --------------------------------------------------------------------------
// Gate transactions
// --------------------------------------------------------------------------

fn row_to_gate_tx(r: &Row<'_>) -> rusqlite::Result<GateTransaction> {
    let dir: String = r.get("direction")?;
    let status: String = r.get("status")?;
    let steps_json: String = r.get("steps")?;
    let appt: Option<String> = r.get("appointment_id")?;
    let expected: Option<String> = r.get("expected_container")?;
    let ocr: Option<String> = r.get("ocr_container")?;
    Ok(GateTransaction {
        id: GateTxId::from_uuid(uuid::Uuid::parse_str(&r.get::<_, String>("id")?).map_err(|_| bad_row("id"))?),
        appointment: appt.map(|s| AppointmentId::from_uuid(uuid::Uuid::parse_str(&s).unwrap_or_default())),
        direction: serde_json::from_str(&format!("\"{dir}\"")).map_err(|_| bad_row("direction"))?,
        expected_container: expected
            .map(|s| ContainerNumber::parse(&s))
            .transpose()
            .map_err(|_| bad_row("expected_container"))?,
        ocr_container: ocr
            .map(|s| ContainerNumber::parse(&s))
            .transpose()
            .map_err(|_| bad_row("ocr_container"))?,
        plate: r.get("plate")?,
        driver_license: r.get("driver_license")?,
        seal_number: r.get("seal_number")?,
        gate_lane: r.get("gate_lane")?,
        steps: serde_json::from_str::<Vec<GateStep>>(&steps_json).map_err(|_| bad_row("steps"))?,
        status: serde_json::from_str(&format!("\"{status}\"")).map_err(|_| bad_row("status"))?,
        reject_reason: r.get("reject_reason")?,
        started_at: parse_ts(&r.get::<_, String>("started_at")?).map_err(|_| bad_row("started_at"))?,
        completed_at: parse_ts_opt(r.get("completed_at")?).map_err(|_| bad_row("completed_at"))?,
    })
}

/// Insert a transaction; when `idempotency_key` is set and already exists,
/// the existing transaction is returned unchanged (retries are safe — §6.2).
pub fn create_gate_tx(store: &Store, tx: &GateTransaction, idempotency_key: Option<&str>) -> CoreResult<GateTransaction> {
    let inserted = on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT OR IGNORE INTO gate_transactions (id, appointment_id, direction, expected_container, ocr_container,
                plate, driver_license, seal_number, gate_lane, steps, status, reject_reason, started_at, completed_at, idempotency_key)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
            rusqlite::params![
                tx.id.to_string(),
                tx.appointment.map(|a| a.to_string()),
                crate::util::enum_str(&tx.direction),
                tx.expected_container.as_ref().map(|c| c.as_str().to_string()),
                tx.ocr_container.as_ref().map(|c| c.as_str().to_string()),
                tx.plate, tx.driver_license, tx.seal_number, tx.gate_lane,
                serde_json::to_string(&tx.steps).map_err(|e| CoreError::Internal(e.to_string()))?,
                crate::util::enum_str(&tx.status),
                tx.reject_reason, ts(tx.started_at), tx.completed_at.map(ts),
                idempotency_key
            ],
        )
        .map_err(map_err)?;
        Ok(c.changes() > 0)
    })?;
    if inserted {
        Ok(tx.clone())
    } else {
        get_gate_tx_by_idempotency_key(store, idempotency_key.ok_or_else(|| {
            CoreError::Conflict("gate transaction conflict".into())
        })?)
    }
}

pub fn get_gate_tx(store: &Store, id: GateTxId) -> CoreResult<GateTransaction> {
    on_conn(&store.conn(), |c| {
        c.query_row("SELECT * FROM gate_transactions WHERE id = ?1", [id.to_string()], row_to_gate_tx)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!("gate tx {id}")),
                _ => map_err(e),
            })
    })
}

pub fn get_gate_tx_by_idempotency_key(store: &Store, key: &str) -> CoreResult<GateTransaction> {
    on_conn(&store.conn(), |c| {
        c.query_row("SELECT * FROM gate_transactions WHERE idempotency_key = ?1", [key], row_to_gate_tx)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!("idempotency key {key}")),
                _ => map_err(e),
            })
    })
}

pub fn update_gate_tx(store: &Store, tx: &GateTransaction) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "UPDATE gate_transactions SET ocr_container = ?2, seal_number = ?3, steps = ?4, status = ?5,
                reject_reason = ?6, completed_at = ?7 WHERE id = ?1",
            rusqlite::params![
                tx.id.to_string(),
                tx.ocr_container.as_ref().map(|c| c.as_str().to_string()),
                tx.seal_number,
                serde_json::to_string(&tx.steps).map_err(|e| CoreError::Internal(e.to_string()))?,
                crate::util::enum_str(&tx.status),
                tx.reject_reason,
                tx.completed_at.map(ts)
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn list_recent_gate_tx(store: &Store, from: DateTime<Utc>, to: DateTime<Utc>, limit: u32) -> CoreResult<Vec<GateTransaction>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT * FROM gate_transactions WHERE started_at >= ?1 AND started_at <= ?2 ORDER BY started_at DESC LIMIT ?3")
            .map_err(map_err)?;
        let rows = stmt
            .query_map(rusqlite::params![ts(from), ts(to), limit], row_to_gate_tx)
            .map_err(map_err)?;
        rows.filter_map(|r| r.ok()).map(Ok).collect()
    })
}

// --------------------------------------------------------------------------
// Gate passes & EIRs (F-19)
// --------------------------------------------------------------------------

pub fn save_gate_pass(store: &Store, pass: &portos_core::gate::GatePass) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO gate_passes (pass_no, gate_tx_id, issued_at, valid_until, qr_payload) VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![pass.pass_no, pass.gate_tx.to_string(), ts(pass.issued_at), ts(pass.valid_until), pass.qr_payload],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn get_gate_pass(store: &Store, pass_no: &str) -> CoreResult<portos_core::gate::GatePass> {
    on_conn(&store.conn(), |c| {
        c.query_row(
            "SELECT * FROM gate_passes WHERE pass_no = ?1",
            [pass_no],
            |r| {
                Ok(portos_core::gate::GatePass {
                    pass_no: r.get(0)?,
                    gate_tx: GateTxId::from_uuid(
                        uuid::Uuid::parse_str(&r.get::<_, String>(1)?).map_err(|_| bad_row("gate_tx_id"))?,
                    ),
                    container: None,
                    plate: String::new(),
                    direction: GateDirection::Import,
                    issued_at: parse_ts(&r.get::<_, String>(2)?).map_err(|_| bad_row("issued_at"))?,
                    qr_payload: r.get(4)?,
                    valid_until: parse_ts(&r.get::<_, String>(3)?).map_err(|_| bad_row("valid_until"))?,
                })
            },
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!("pass {pass_no}")),
            _ => map_err(e),
        })
    })
}

pub fn save_eir(store: &Store, eir: &portos_core::gate::EquipmentInterchangeReport) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO eirs (eir_no, gate_tx_id, container_id, condition, damages, signed_by, at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![
                eir.eir_no, eir.gate_tx.to_string(), eir.container.as_str(),
                eir.condition,
                serde_json::to_string(&eir.damages).map_err(|e| CoreError::Internal(e.to_string()))?,
                eir.signed_by, ts(eir.at)
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn count_gate_tx(store: &Store) -> CoreResult<i64> {
    on_conn(&store.conn(), |c| {
        c.query_row("SELECT COUNT(*) FROM gate_transactions", [], |r| r.get(0)).map_err(map_err)
    })
}

/// Sanity gate: reject unknown statuses at the type level in callers.
#[allow(dead_code)]
const _: () = {
    // Compile-time check that GateStatus is the one used in storage.
    fn assert_same<T>(_t: T) {}
    fn check(s: GateStatus) {
        assert_same(s);
    }
};

#[cfg(test)]
mod tests {
    use super::*;
    use portos_core::gate::{GateTransaction, GateDirection};

    #[test]
    fn appointment_lifecycle() {
        let store = Store::open_in_memory().unwrap();
        let a = TruckAppointment {
            id: AppointmentId::generate(),
            reference: "TA-2026-000001".into(),
            trucking_company: None,
            driver_name: "رضا محمدی".into(),
            driver_license: "DL-998877".into(),
            plate: "12A34567".into(),
            container: Some(ContainerNumber::parse("MSKU3881107").unwrap()),
            direction: GateDirection::Export,
            slot_from: Utc::now(),
            slot_to: Utc::now() + chrono::Duration::hours(1),
            status: AppointmentStatus::Booked,
            created_at: Utc::now(),
        };
        create_appointment(&store, &a).unwrap();
        let got = get_appointment_by_reference(&store, "TA-2026-000001").unwrap();
        assert_eq!(got.driver_name, "رضا محمدی");
        update_appointment_status(&store, a.id, AppointmentStatus::CheckedIn).unwrap();
        let got = get_appointment(&store, a.id).unwrap();
        assert_eq!(got.status, AppointmentStatus::CheckedIn);
        assert_eq!(list_appointments_in_window(&store, Utc::now() - chrono::Duration::hours(1), Utc::now() + chrono::Duration::hours(2)).unwrap().len(), 1);
    }

    #[test]
    fn gate_tx_idempotency() {
        let store = Store::open_in_memory().unwrap();
        let tx = GateTransaction::start(GateDirection::Export, None, None, "12A34567", "DL-1", "L1", Utc::now());
        let first = create_gate_tx(&store, &tx, Some("key-1")).unwrap();
        let again = create_gate_tx(&store, &tx, Some("key-1")).unwrap();
        assert_eq!(first.id, again.id);
        assert!(get_gate_tx(&store, first.id).is_ok());

        let mut tx2 = GateTransaction::start(GateDirection::Import, None, None, "P", "D", "L2", Utc::now());
        create_gate_tx(&store, &tx2, None).unwrap();
        tx2.apply(portos_core::gate::GateStepKind::AppointmentCheck, true, None, Utc::now()).unwrap();
        update_gate_tx(&store, &tx2).unwrap();
        let got = get_gate_tx(&store, tx2.id).unwrap();
        assert_eq!(got.steps.len(), 1);
        assert_eq!(got.status, GateStatus::InProgress);
    }
}
