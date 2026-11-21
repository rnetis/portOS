//! Equipment, jobs and move records (PRD F-13..F-15).

use crate::util::{map_err, on_conn, parse_ts, parse_ts_opt, ts};
use crate::Store;
use chrono::{DateTime, Utc};
use portos_core::container::ContainerNumber;
use portos_core::ids::{CallId, EquipmentId, JobId, MoveId};
use portos_core::work::{Equipment, EquipmentStatus, Job, JobStatus, MoveRecord};
use portos_core::{CoreError, CoreResult};
use rusqlite::{OptionalExtension, Row};

fn bad_row(col: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(0, col.to_string(), rusqlite::types::Type::Text)
}

fn row_to_equipment(r: &Row<'_>) -> rusqlite::Result<Equipment> {
    let kind: String = r.get("kind")?;
    let status: String = r.get("status")?;
    let telemetry: Option<String> = r.get("telemetry")?;
    Ok(Equipment {
        id: EquipmentId::from_uuid(
            uuid::Uuid::parse_str(&r.get::<_, String>("id")?).map_err(|_| bad_row("id"))?,
        ),
        code: r.get("code")?,
        kind: serde_json::from_str(&format!("\"{kind}\"")).map_err(|_| bad_row("kind"))?,
        status: serde_json::from_str(&format!("\"{status}\"")).map_err(|_| bad_row("status"))?,
        hours_meter: r.get("hours_meter")?,
        last_service_at: parse_ts_opt(r.get("last_service_at")?)
            .map_err(|_| bad_row("last_service_at"))?,
        telemetry: telemetry.and_then(|t| serde_json::from_str(&t).ok()),
    })
}

pub fn upsert_equipment(store: &Store, e: &Equipment) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO equipment (id, code, kind, status, hours_meter, last_service_at, telemetry)
             VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(code) DO UPDATE SET kind=excluded.kind, status=excluded.status,
                hours_meter=excluded.hours_meter, last_service_at=excluded.last_service_at,
                telemetry=excluded.telemetry",
            rusqlite::params![
                e.id.to_string(), e.code,
                crate::util::enum_str(&e.kind),
                crate::util::enum_str(&e.status),
                e.hours_meter, e.last_service_at.map(ts),
                e.telemetry.as_ref().map(|t| t.to_string())
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn list_equipment(store: &Store) -> CoreResult<Vec<Equipment>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT * FROM equipment ORDER BY code")
            .map_err(map_err)?;
        let rows = stmt.query_map([], row_to_equipment).map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
    })
}

/// Id of a synthetic equipment row used for externally reported moves
/// (e.g. COARRI crane reports) that carry no terminal equipment id. The
/// row keeps the moves table's referential integrity intact.
pub fn ensure_report_equipment(store: &Store, code: &str) -> CoreResult<EquipmentId> {
    on_conn(&store.conn(), |c| {
        if let Some(raw) = c
            .query_row("SELECT id FROM equipment WHERE code = ?1", [code], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .map_err(map_err)?
        {
            let uuid = uuid::Uuid::parse_str(&raw)
                .map_err(|e| CoreError::Internal(format!("equipment id: {e}")))?;
            return Ok(EquipmentId::from_uuid(uuid));
        }
        let id = EquipmentId::generate();
        c.execute(
            "INSERT INTO equipment (id, code, kind, status, hours_meter) VALUES (?1,?2,'quay_crane','offline',0)",
            rusqlite::params![id.to_string(), code],
        )
        .map_err(map_err)?;
        Ok(id)
    })
}

pub fn update_equipment_status(
    store: &Store,
    id: EquipmentId,
    status: EquipmentStatus,
) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "UPDATE equipment SET status = ?2 WHERE id = ?1",
            rusqlite::params![id.to_string(), crate::util::enum_str(&status)],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn update_telemetry(
    store: &Store,
    code: &str,
    telemetry: &serde_json::Value,
    hours: Option<f64>,
) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        if let Some(h) = hours {
            c.execute(
                "UPDATE equipment SET telemetry = ?2, hours_meter = ?3 WHERE code = ?1",
                rusqlite::params![code, telemetry.to_string(), h],
            )
            .map_err(map_err)?;
        } else {
            c.execute(
                "UPDATE equipment SET telemetry = ?2 WHERE code = ?1",
                rusqlite::params![code, telemetry.to_string()],
            )
            .map_err(map_err)?;
        }
        Ok(())
    })
}

fn row_to_job(r: &Row<'_>) -> rusqlite::Result<Job> {
    let kind: String = r.get("kind")?;
    let status: String = r.get("status")?;
    let call: Option<String> = r.get("call_id")?;
    let container: Option<String> = r.get("container_id")?;
    let assigned: Option<String> = r.get("assigned_to")?;
    Ok(Job {
        id: JobId::from_uuid(
            uuid::Uuid::parse_str(&r.get::<_, String>("id")?).map_err(|_| bad_row("id"))?,
        ),
        kind: serde_json::from_str(&format!("\"{kind}\"")).map_err(|_| bad_row("kind"))?,
        call: call.map(|s| CallId::from_uuid(uuid::Uuid::parse_str(&s).unwrap_or_default())),
        container: container
            .map(|s| ContainerNumber::parse(&s))
            .transpose()
            .map_err(|_| bad_row("container_id"))?,
        assigned_to: assigned
            .map(|s| EquipmentId::from_uuid(uuid::Uuid::parse_str(&s).unwrap_or_default())),
        priority: r.get::<_, i64>("priority")? as u8,
        status: serde_json::from_str(&format!("\"{status}\"")).map_err(|_| bad_row("status"))?,
        created_at: parse_ts(&r.get::<_, String>("created_at")?)
            .map_err(|_| bad_row("created_at"))?,
        assigned_at: parse_ts_opt(r.get("assigned_at")?).map_err(|_| bad_row("assigned_at"))?,
        completed_at: parse_ts_opt(r.get("completed_at")?).map_err(|_| bad_row("completed_at"))?,
        issue: r.get("issue")?,
    })
}

pub fn create_job(store: &Store, job: &Job) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO jobs (id, kind, call_id, container_id, assigned_to, priority, status, created_at, assigned_at, completed_at, issue)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            rusqlite::params![
                job.id.to_string(),
                crate::util::enum_str(&job.kind),
                job.call.map(|c| c.to_string()),
                job.container.as_ref().map(|c| c.as_str().to_string()),
                job.assigned_to.map(|e| e.to_string()),
                job.priority,
                crate::util::enum_str(&job.status),
                ts(job.created_at), job.assigned_at.map(ts), job.completed_at.map(ts),
                job.issue
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn get_job(store: &Store, id: JobId) -> CoreResult<Job> {
    on_conn(&store.conn(), |c| {
        c.query_row(
            "SELECT * FROM jobs WHERE id = ?1",
            [id.to_string()],
            row_to_job,
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!("job {id}")),
            _ => map_err(e),
        })
    })
}

pub fn update_job(store: &Store, job: &Job) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "UPDATE jobs SET assigned_to = ?2, status = ?3, assigned_at = ?4, completed_at = ?5, issue = ?6 WHERE id = ?1",
            rusqlite::params![
                job.id.to_string(),
                job.assigned_to.map(|e| e.to_string()),
                crate::util::enum_str(&job.status),
                job.assigned_at.map(ts), job.completed_at.map(ts),
                job.issue
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn list_jobs(store: &Store, statuses: &[JobStatus], limit: u32) -> CoreResult<Vec<Job>> {
    if statuses.is_empty() {
        return on_conn(&store.conn(), |c| {
            let mut stmt = c
                .prepare("SELECT * FROM jobs ORDER BY priority, created_at LIMIT ?1")
                .map_err(map_err)?;
            let rows = stmt.query_map([limit], row_to_job).map_err(map_err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
        });
    }
    // Simple two-status fast path plus generic IN list.
    let placeholders: Vec<String> = statuses
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect();
    let sql = format!(
        "SELECT * FROM jobs WHERE status IN ({}) ORDER BY priority, created_at LIMIT ?{}",
        placeholders.join(","),
        statuses.len() + 1
    );
    let params: Vec<String> = statuses
        .iter()
        .map(|s| Ok(crate::util::enum_str(s)))
        .collect::<CoreResult<_>>()?;
    on_conn(&store.conn(), |c| {
        let mut stmt = c.prepare(&sql).map_err(map_err)?;
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = params
            .iter()
            .map(|p| Box::new(p.clone()) as Box<dyn rusqlite::ToSql>)
            .collect();
        binds.push(Box::new(limit));
        let pref: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(pref.as_slice(), row_to_job)
            .map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
    })
}

fn row_to_move(r: &Row<'_>) -> rusqlite::Result<MoveRecord> {
    Ok(MoveRecord {
        id: MoveId::from_uuid(
            uuid::Uuid::parse_str(&r.get::<_, String>("id")?).map_err(|_| bad_row("id"))?,
        ),
        job: JobId::from_uuid(
            uuid::Uuid::parse_str(&r.get::<_, String>("job_id")?).map_err(|_| bad_row("job_id"))?,
        ),
        equipment: EquipmentId::from_uuid(
            uuid::Uuid::parse_str(&r.get::<_, String>("equipment_id")?)
                .map_err(|_| bad_row("equipment_id"))?,
        ),
        container: ContainerNumber::parse(&r.get::<_, String>("container_id")?)
            .map_err(|_| bad_row("container_id"))?,
        kind: serde_json::from_str(&format!("\"{}\"", r.get::<_, String>("kind")?))
            .map_err(|_| bad_row("kind"))?,
        started_at: parse_ts(&r.get::<_, String>("started_at")?)
            .map_err(|_| bad_row("started_at"))?,
        finished_at: parse_ts(&r.get::<_, String>("finished_at")?)
            .map_err(|_| bad_row("finished_at"))?,
        success: r.get::<_, i64>("success")? != 0,
    })
}

pub fn record_move(store: &Store, m: &MoveRecord) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO moves (id, job_id, equipment_id, container_id, kind, started_at, finished_at, success)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                m.id.to_string(), m.job.to_string(), m.equipment.to_string(),
                m.container.as_str(),
                crate::util::enum_str(&m.kind),
                ts(m.started_at), ts(m.finished_at), i64::from(m.success)
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

/// Number of successful moves belonging to a vessel call (via jobs — F-28).
pub fn count_moves_for_call(store: &Store, call: portos_core::ids::CallId) -> CoreResult<u32> {
    on_conn(&store.conn(), |c| {
        let n: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM moves m JOIN jobs j ON j.id = m.job_id
                 WHERE j.call_id = ?1 AND m.success = 1",
                [call.to_string()],
                |r| r.get(0),
            )
            .map_err(map_err)?;
        Ok(n as u32)
    })
}

pub fn moves_between(
    store: &Store,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> CoreResult<Vec<MoveRecord>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT * FROM moves WHERE started_at >= ?1 AND finished_at <= ?2 ORDER BY started_at")
            .map_err(map_err)?;
        let rows = stmt
            .query_map(rusqlite::params![ts(from), ts(to)], row_to_move)
            .map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use portos_core::work::{EquipmentKind, JobKind};

    #[test]
    fn equipment_jobs_moves_flow() {
        let store = Store::open_in_memory().unwrap();
        let eq = Equipment {
            id: EquipmentId::generate(),
            code: "QC-01".into(),
            kind: EquipmentKind::QuayCrane,
            status: EquipmentStatus::Available,
            hours_meter: 1234.5,
            last_service_at: None,
            telemetry: None,
        };
        upsert_equipment(&store, &eq).unwrap();
        update_telemetry(
            &store,
            "QC-01",
            &serde_json::json!({"spread_load_pct": 72}),
            Some(1240.0),
        )
        .unwrap();
        let eqs = list_equipment(&store).unwrap();
        assert_eq!(eqs.len(), 1);
        assert_eq!(eqs[0].hours_meter, 1240.0);

        let job = Job {
            id: JobId::generate(),
            kind: JobKind::Discharge,
            call: None,
            container: Some(ContainerNumber::parse("MSKU3881107").unwrap()),
            assigned_to: None,
            priority: 1,
            status: JobStatus::Queued,
            created_at: Utc::now(),
            assigned_at: None,
            completed_at: None,
            issue: None,
        };
        create_job(&store, &job).unwrap();
        let queued = list_jobs(&store, &[JobStatus::Queued], 10).unwrap();
        assert_eq!(queued.len(), 1);

        let mut j = get_job(&store, job.id).unwrap();
        j.status = JobStatus::InProgress;
        j.assigned_to = Some(eq.id);
        j.assigned_at = Some(Utc::now());
        update_job(&store, &j).unwrap();
        assert_eq!(
            get_job(&store, job.id).unwrap().status,
            JobStatus::InProgress
        );

        let mv = MoveRecord {
            id: MoveId::generate(),
            job: job.id,
            equipment: eq.id,
            container: job.container.unwrap(),
            kind: JobKind::Discharge,
            started_at: Utc::now(),
            finished_at: Utc::now() + chrono::Duration::seconds(90),
            success: true,
        };
        record_move(&store, &mv).unwrap();
        assert_eq!(
            moves_between(
                &store,
                Utc::now() - chrono::Duration::minutes(1),
                Utc::now() + chrono::Duration::minutes(5)
            )
            .unwrap()
            .len(),
            1
        );
    }
}
