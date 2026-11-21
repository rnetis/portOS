//! Fleet repositories: vessels, vessel calls, berths, berth requests and
//! allocations (PRD F-1, F-2, F-5, F-47).

use crate::util::{map_err, on_conn, parse_ts, parse_ts_opt, ts};
use crate::Store;
use chrono::{DateTime, Utc};
use portos_core::berth::{Berth, BerthAllocation, CargoType};
use portos_core::container::IsoSizeType;
use portos_core::ids::{AllocationId, BerthId, CallId, CustomerId, VesselId};
use portos_core::vessel::{
    CallStatus, ImoNumber, UnLocode, Vessel, VesselCall,
};
use portos_core::{CoreError, CoreResult};
use rusqlite::Row;

fn bad_row(col: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(0, col.to_string(), rusqlite::types::Type::Text)
}

fn parse_uuid(s: String, col: &str) -> rusqlite::Result<uuid::Uuid> {
    uuid::Uuid::parse_str(&s).map_err(|_| bad_row(col))
}

// --------------------------------------------------------------------------
// Vessels
// --------------------------------------------------------------------------

fn row_to_vessel(r: &Row<'_>) -> rusqlite::Result<Vessel> {
    let imo: String = r.get("imo")?;
    let loa: f64 = r.get("loa_m")?;
    let customer: Option<String> = r.get("customer_id")?;
    Ok(Vessel {
        id: VesselId::from_uuid(parse_uuid(r.get("id")?, "id")?),
        imo: ImoNumber::parse(&imo).map_err(|_| bad_row("imo"))?,
        name_en: r.get("name_en")?,
        name_fa: r.get("name_fa")?,
        mmsi: r.get("mmsi")?,
        flag_code: r.get("flag_code")?,
        loa_m: loa,
        beam_m: r.get("beam_m")?,
        max_draft_m: r.get("max_draft_m")?,
        dwt_t: r.get::<_, i64>("dwt_t")? as u32,
        reefer_points: r.get::<_, i64>("reefer_points")? as u16,
        gearless: r.get::<_, i64>("gearless")? != 0,
        customer: customer.map(|s| CustomerId::from_uuid(parse_uuid(s, "customer_id").ok().unwrap_or_default())),
    })
}

pub fn create_vessel(store: &Store, v: &Vessel) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO vessels (id, imo, name_en, name_fa, mmsi, flag_code, loa_m, beam_m, max_draft_m, dwt_t, reefer_points, gearless, customer_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            rusqlite::params![
                v.id.to_string(), v.imo.as_str(), v.name_en, v.name_fa,
                v.mmsi, v.flag_code, v.loa_m, v.beam_m, v.max_draft_m,
                v.dwt_t, v.reefer_points, i64::from(v.gearless),
                v.customer.map(|c| c.to_string())
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn list_vessels(store: &Store) -> CoreResult<Vec<Vessel>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c.prepare("SELECT * FROM vessels ORDER BY name_en").map_err(map_err)?;
        let rows = stmt.query_map([], row_to_vessel).map_err(map_err)?;
        rows.filter_map(|r| r.ok()).map(Ok).collect()
    })
}

pub fn get_vessel(store: &Store, id: VesselId) -> CoreResult<Vessel> {
    on_conn(&store.conn(), |c| {
        c.query_row("SELECT * FROM vessels WHERE id = ?1", [id.to_string()], row_to_vessel)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!("vessel {id}")),
                _ => map_err(e),
            })
    })
}

pub fn get_vessel_by_imo(store: &Store, imo: &str) -> CoreResult<Option<Vessel>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c.prepare("SELECT * FROM vessels WHERE imo = ?1").map_err(map_err)?;
        let mut rows = stmt.query_map([imo], row_to_vessel).map_err(map_err)?;
        match rows.next() {
            Some(row) => Ok(Some(row.map_err(map_err)?)),
            None => Ok(None),
        }
    })
}

// --------------------------------------------------------------------------
// Vessel calls
// --------------------------------------------------------------------------

fn row_to_call(r: &Row<'_>) -> rusqlite::Result<VesselCall> {
    let status: String = r.get("status")?;
    let cargo_json: String = r.get("cargo_mix")?;
    let eta: String = r.get("eta_utc")?;
    let etd: Option<String> = r.get("etd_utc")?;
    Ok(VesselCall {
        id: CallId::from_uuid(parse_uuid(r.get("id")?, "id")?),
        vessel: VesselId::from_uuid(parse_uuid(r.get("vessel_id")?, "vessel_id")?),
        voyage_code: r.get("voyage_code")?,
        port: UnLocode::parse(&r.get::<_, String>("port")?).map_err(|_| bad_row("port"))?,
        eta_utc: parse_ts(&eta).map_err(|_| bad_row("eta_utc"))?,
        etd_utc: parse_ts_opt(etd).map_err(|_| bad_row("etd_utc"))?,
        status: serde_json::from_str::<CallStatus>(&format!("\"{status}\"")).map_err(|_| bad_row(&format!("status raw={status:?}")))?,
        cargo_mix: serde_json::from_str(&cargo_json).map_err(|_| bad_row("cargo_mix"))?,
        anchorage_arrived_at: parse_ts_opt(r.get("anchorage_arrived_at")?).map_err(|_| bad_row("anchorage_arrived_at"))?,
        berthed_at: parse_ts_opt(r.get("berthed_at")?).map_err(|_| bad_row("berthed_at"))?,
        work_completed_at: parse_ts_opt(r.get("work_completed_at")?).map_err(|_| bad_row("work_completed_at"))?,
        departed_at: parse_ts_opt(r.get("departed_at")?).map_err(|_| bad_row("departed_at"))?,
        created_by: r.get("created_by")?,
        created_at: parse_ts(&r.get::<_, String>("created_at")?).map_err(|_| bad_row("created_at"))?,
        updated_at: parse_ts(&r.get::<_, String>("updated_at")?).map_err(|_| bad_row("updated_at"))?,
    })
}

pub fn create_call(store: &Store, call: &VesselCall) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO vessel_calls (id, vessel_id, voyage_code, port, eta_utc, etd_utc, status, cargo_mix,
                anchorage_arrived_at, berthed_at, work_completed_at, departed_at, created_by, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14)",
            rusqlite::params![
                call.id.to_string(), call.vessel.to_string(), call.voyage_code, call.port.as_str(),
                ts(call.eta_utc), call.etd_utc.map(ts),
                crate::util::enum_str(&call.status),
                serde_json::to_string(&call.cargo_mix).map_err(|e| CoreError::Internal(e.to_string()))?,
                call.anchorage_arrived_at.map(ts), call.berthed_at.map(ts),
                call.work_completed_at.map(ts), call.departed_at.map(ts),
                call.created_by, ts(call.created_at)
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn get_call(store: &Store, id: CallId) -> CoreResult<VesselCall> {
    on_conn(&store.conn(), |c| {
        c.query_row("SELECT * FROM vessel_calls WHERE id = ?1", [id.to_string()], row_to_call)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!("call {id}")),
                _ => map_err(e),
            })
    })
}

pub fn list_calls(store: &Store, status: Option<CallStatus>, from: Option<DateTime<Utc>>, to: Option<DateTime<Utc>>, limit: u32) -> CoreResult<Vec<VesselCall>> {
    on_conn(&store.conn(), |c| {
        let mut sql = String::from("SELECT * FROM vessel_calls WHERE 1=1");
        if status.is_some() {
            sql.push_str(" AND status = ?1");
        }
        if from.is_some() {
            sql.push_str(&format!(" AND eta_utc >= ?{}", if status.is_some() { 2 } else { 1 }));
        }
        if to.is_some() {
            let base = if status.is_some() { 1 } else { 0 } + if from.is_some() { 1 } else { 0 };
            sql.push_str(&format!(" AND eta_utc <= ?{}", base + 1));
        }
        let base = if status.is_some() { 1 } else { 0 } + if from.is_some() { 1 } else { 0 } + if to.is_some() { 1 } else { 0 };
        sql.push_str(&format!(" ORDER BY eta_utc DESC LIMIT ?{}", base + 1));

        let mut stmt = c.prepare(&sql).map_err(map_err)?;
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(s) = &status {
            params.push(Box::new(crate::util::enum_str(s)));
        }
        if let Some(f) = from {
            params.push(Box::new(ts(f)));
        }
        if let Some(t) = to {
            params.push(Box::new(ts(t)));
        }
        params.push(Box::new(limit));
        let pref: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref() as &dyn rusqlite::ToSql).collect();
        let rows = stmt.query_map(pref.as_slice(), row_to_call).map_err(map_err)?;
        rows.filter_map(|r| r.ok()).map(Ok).collect()
    })
}

pub fn update_call_status(store: &Store, id: CallId, status: CallStatus, stamp: Option<DateTime<Utc>>) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        let status_str = crate::util::enum_str(&status);
        let (col, col_val): (&str, Option<String>) = match status {
            CallStatus::AnchorageArrived => ("anchorage_arrived_at", stamp.map(ts)),
            CallStatus::Berthed => ("berthed_at", stamp.map(ts)),
            CallStatus::Completed => ("work_completed_at", stamp.map(ts)),
            CallStatus::Departed => ("departed_at", stamp.map(ts)),
            _ => ("updated_at", None),
        };
        c.execute(
            &format!("UPDATE vessel_calls SET status = ?2, updated_at = ?3, {col} = COALESCE(?4, {col}) WHERE id = ?1"),
            rusqlite::params![id.to_string(), status_str, ts(Utc::now()), col_val],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn update_call_eta(store: &Store, id: CallId, eta: DateTime<Utc>, etd: Option<DateTime<Utc>>) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "UPDATE vessel_calls SET eta_utc = ?2, etd_utc = COALESCE(?3, etd_utc), updated_at = ?4 WHERE id = ?1",
            rusqlite::params![id.to_string(), ts(eta), etd.map(ts), ts(Utc::now())],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

// --------------------------------------------------------------------------
// Berths
// --------------------------------------------------------------------------

fn row_to_berth(r: &Row<'_>) -> rusqlite::Result<Berth> {
    let caps_json: String = r.get("capabilities")?;
    let caps: Vec<CargoType> = serde_json::from_str(&caps_json).unwrap_or_default();
    Ok(Berth {
        id: BerthId::from_uuid(parse_uuid(r.get("id")?, "id")?),
        code: r.get("code")?,
        name_en: r.get("name_en")?,
        name_fa: r.get("name_fa")?,
        length_m: r.get("length_m")?,
        max_draught_m: r.get("max_draught_m")?,
        crane_count: r.get::<_, i64>("crane_count")? as u8,
        shore_power: r.get::<_, i64>("shore_power")? != 0,
        capabilities: caps,
        active: r.get::<_, i64>("active")? != 0,
    })
}

pub fn create_berth(store: &Store, b: &Berth) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO berths (id, code, name_en, name_fa, length_m, max_draught_m, crane_count, shore_power, capabilities, active)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            rusqlite::params![
                b.id.to_string(), b.code, b.name_en, b.name_fa, b.length_m,
                b.max_draught_m, b.crane_count, i64::from(b.shore_power),
                serde_json::to_string(&b.capabilities).map_err(|e| CoreError::Internal(e.to_string()))?,
                i64::from(b.active)
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn list_berths(store: &Store) -> CoreResult<Vec<Berth>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c.prepare("SELECT * FROM berths ORDER BY code").map_err(map_err)?;
        let rows = stmt.query_map([], row_to_berth).map_err(map_err)?;
        rows.filter_map(|r| r.ok()).map(Ok).collect()
    })
}

pub fn get_berth(store: &Store, id: BerthId) -> CoreResult<Berth> {
    on_conn(&store.conn(), |c| {
        c.query_row("SELECT * FROM berths WHERE id = ?1", [id.to_string()], row_to_berth)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!("berth {id}")),
                _ => map_err(e),
            })
    })
}

// --------------------------------------------------------------------------
// Berth requests & allocations
// --------------------------------------------------------------------------

pub fn create_berth_request(
    store: &Store,
    id: AllocationId,
    call: CallId,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    priority: u8,
    reason: Option<&str>,
) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO berth_requests (id, call_id, requested_from, requested_to, priority, reason, status, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,'open',?7)",
            rusqlite::params![id.to_string(), call.to_string(), ts(from), ts(to), priority, reason, ts(Utc::now())],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn open_request_for_call(store: &Store, call: CallId) -> CoreResult<Option<(AllocationId, DateTime<Utc>, DateTime<Utc>, u8, Option<String>)>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT id, requested_from, requested_to, priority, reason FROM berth_requests WHERE call_id = ?1 AND status = 'open' ORDER BY created_at DESC LIMIT 1")
            .map_err(map_err)?;
        let mut rows = stmt.query_map([call.to_string()], |r| {
            let id_s: String = r.get(0)?;
            let from_s: String = r.get(1)?;
            let to_s: String = r.get(2)?;
            let priority: i64 = r.get(3)?;
            let reason: Option<String> = r.get(4)?;
            Ok((
                AllocationId::from_uuid(parse_uuid(id_s, "id")?),
                parse_ts(&from_s).map_err(|_| bad_row("requested_from"))?,
                parse_ts(&to_s).map_err(|_| bad_row("requested_to"))?,
                priority.clamp(1, 5) as u8,
                reason,
            ))
        })
        .map_err(map_err)?;
        match rows.next() {
            Some(row) => Ok(Some(row.map_err(map_err)?)),
            None => Ok(None),
        }
    })
}

pub fn mark_request_allocated(store: &Store, request: AllocationId) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute("UPDATE berth_requests SET status = 'allocated' WHERE id = ?1", [request.to_string()])
            .map_err(map_err)?;
        Ok(())
    })
}

fn row_to_allocation(r: &Row<'_>) -> rusqlite::Result<BerthAllocation> {
    let reasons_json: String = r.get("reasons")?;
    Ok(BerthAllocation {
        id: AllocationId::from_uuid(parse_uuid(r.get("id")?, "id")?),
        call: CallId::from_uuid(parse_uuid(r.get("call_id")?, "call_id")?),
        berth: BerthId::from_uuid(parse_uuid(r.get("berth_id")?, "berth_id")?),
        planned_from: parse_ts(&r.get::<_, String>("planned_from")?).map_err(|_| bad_row("planned_from"))?,
        planned_to: parse_ts(&r.get::<_, String>("planned_to")?).map_err(|_| bad_row("planned_to"))?,
        score: r.get("score")?,
        reasons: serde_json::from_str(&reasons_json).unwrap_or_default(),
        decided_by: r.get("decided_by")?,
        decided_at: parse_ts(&r.get::<_, String>("decided_at")?).map_err(|_| bad_row("decided_at"))?,
    })
}

pub fn create_allocation(store: &Store, a: &BerthAllocation) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO berth_allocations (id, call_id, berth_id, planned_from, planned_to, score, reasons, decided_by, decided_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            rusqlite::params![
                a.id.to_string(), a.call.to_string(), a.berth.to_string(),
                ts(a.planned_from), ts(a.planned_to), a.score,
                serde_json::to_string(&a.reasons).map_err(|e| CoreError::Internal(e.to_string()))?,
                a.decided_by, ts(a.decided_at)
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn list_allocations_for_call(store: &Store, call: CallId) -> CoreResult<Vec<BerthAllocation>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT * FROM berth_allocations WHERE call_id = ?1 ORDER BY decided_at DESC")
            .map_err(map_err)?;
        let rows = stmt.query_map([call.to_string()], row_to_allocation).map_err(map_err)?;
        rows.filter_map(|r| r.ok()).map(Ok).collect()
    })
}

/// Occupancy intervals overlapping `[from, to]` — input to the allocator.
pub fn occupancy_between(store: &Store, from: DateTime<Utc>, to: DateTime<Utc>) -> CoreResult<Vec<(BerthId, DateTime<Utc>, DateTime<Utc>)>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT berth_id, planned_from, planned_to FROM berth_allocations WHERE planned_to > ?1 AND planned_from < ?2")
            .map_err(map_err)?;
        let rows = stmt
            .query_map(rusqlite::params![ts(from), ts(to)], |r| {
                let berth_s: String = r.get(0)?;
                let f_s: String = r.get(1)?;
                let t_s: String = r.get(2)?;
                Ok((
                    BerthId::from_uuid(parse_uuid(berth_s, "berth_id")?),
                    parse_ts(&f_s).map_err(|_| bad_row("planned_from"))?,
                    parse_ts(&t_s).map_err(|_| bad_row("planned_to"))?,
                ))
            })
            .map_err(map_err)?;
        rows.filter_map(|r| r.ok()).map(Ok).collect()
    })
}

/// Size/type helper used by API DTOs.
pub fn normalize_size_type(code: &str) -> CoreResult<IsoSizeType> {
    IsoSizeType::parse(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use portos_core::vessel::CargoMix;

    fn vessel() -> Vessel {
        Vessel {
            id: VesselId::generate(),
            imo: ImoNumber::parse("9811000").unwrap(),
            name_en: "MV Gulf Pearl".into(),
            name_fa: "گلف پرل".into(),
            mmsi: None,
            flag_code: Some("IR".into()),
            loa_m: 294.0,
            beam_m: 32.2,
            max_draft_m: 14.0,
            dwt_t: 93_000,
            reefer_points: 600,
            gearless: true,
            customer: None,
        }
    }

    #[test]
    fn vessel_and_call_round_trip() {
        let store = Store::open_in_memory().unwrap();
        let v = vessel();
        create_vessel(&store, &v).unwrap();
        assert!(get_vessel_by_imo(&store, "9811000").unwrap().is_some());

        let call = VesselCall {
            id: CallId::generate(),
            vessel: v.id,
            voyage_code: "V212E".into(),
            port: UnLocode::parse("IRBND").unwrap(),
            eta_utc: Utc::now() + Duration::hours(24),
            etd_utc: Some(Utc::now() + Duration::hours(48)),
            status: CallStatus::Scheduled,
            cargo_mix: CargoMix { discharge_teu: 1200, load_teu: 800, ..Default::default() },
            anchorage_arrived_at: None,
            berthed_at: None,
            work_completed_at: None,
            departed_at: None,
            created_by: "planner".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        create_call(&store, &call).unwrap();
        let got = get_call(&store, call.id).unwrap();
        assert_eq!(got.cargo_mix.discharge_teu, 1200);
        assert_eq!(got.status, CallStatus::Scheduled);
        update_call_status(&store, call.id, CallStatus::AnchorageArrived, Some(Utc::now())).unwrap();
        let got = get_call(&store, call.id).unwrap();
        assert!(got.anchorage_arrived_at.is_some());
        assert_eq!(got.status, CallStatus::AnchorageArrived);
        let listed = list_calls(&store, Some(CallStatus::AnchorageArrived), None, None, 10).unwrap();
        assert_eq!(listed.len(), 1);
    }

    #[test]
    fn duplicate_voyage_conflicts() {
        let store = Store::open_in_memory().unwrap();
        let v = vessel();
        create_vessel(&store, &v).unwrap();
        let mk = || VesselCall {
            id: CallId::generate(),
            vessel: v.id,
            voyage_code: "SAME".into(),
            port: UnLocode::parse("IRBND").unwrap(),
            eta_utc: Utc::now(),
            etd_utc: None,
            status: CallStatus::Scheduled,
            cargo_mix: CargoMix::default(),
            anchorage_arrived_at: None,
            berthed_at: None,
            work_completed_at: None,
            departed_at: None,
            created_by: "x".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        create_call(&store, &mk()).unwrap();
        assert!(create_call(&store, &mk()).is_err());
    }

    #[test]
    fn berth_and_occupancy() {
        let store = Store::open_in_memory().unwrap();
        let b = Berth {
            id: BerthId::generate(),
            code: "B1".into(),
            name_en: "Berth 1".into(),
            name_fa: "اسکله ۱".into(),
            length_m: 320.0,
            max_draught_m: 15.0,
            crane_count: 4,
            shore_power: true,
            capabilities: vec![CargoType::Container],
            active: true,
        };
        create_berth(&store, &b).unwrap();
        assert_eq!(list_berths(&store).unwrap().len(), 1);

        let v = vessel();
        create_vessel(&store, &v).unwrap();
        let call = CallId::generate();
        create_call(&store, &VesselCall {
            id: call,
            vessel: v.id,
            voyage_code: "V001".into(),
            port: UnLocode::parse("IRBND").unwrap(),
            eta_utc: Utc::now(),
            etd_utc: None,
            status: CallStatus::Scheduled,
            cargo_mix: CargoMix::default(),
            anchorage_arrived_at: None,
            berthed_at: None,
            work_completed_at: None,
            departed_at: None,
            created_by: "tester".into(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
        .unwrap();
        let req = AllocationId::generate();
        create_berth_request(&store, req, call, Utc::now(), Utc::now() + Duration::hours(24), 2, Some("reefer heavy")).unwrap();
        let (rid, _, _, prio, reason) = open_request_for_call(&store, call).unwrap().unwrap();
        assert_eq!(rid, req);
        assert_eq!(prio, 2);
        assert_eq!(reason.as_deref(), Some("reefer heavy"));

        let a = BerthAllocation {
            id: AllocationId::generate(),
            call,
            berth: b.id,
            planned_from: Utc::now(),
            planned_to: Utc::now() + Duration::hours(24),
            score: Some(0.83),
            reasons: vec!["LOA fits".into()],
            decided_by: "planner".into(),
            decided_at: Utc::now(),
        };
        create_allocation(&store, &a).unwrap();
        assert_eq!(list_allocations_for_call(&store, call).unwrap().len(), 1);
        let occ = occupancy_between(&store, Utc::now(), Utc::now() + Duration::hours(1)).unwrap();
        assert_eq!(occ.len(), 1);
        mark_request_allocated(&store, req).unwrap();
        assert!(open_request_for_call(&store, call).unwrap().is_none());
    }
}
