//! Yard repositories: blocks, slots, containers, holds, reefer readings and
//! block snapshots (PRD F-7..F-12).

use crate::util::{map_err, on_conn, parse_ts, parse_ts_opt, ts};
use crate::Store;
use chrono::{DateTime, Utc};
use portos_core::container::{
    Container, ContainerNumber, ContainerState, CustomsStatus, Hold, IsoSizeType,
};
use portos_core::ids::{BlockId, CustomerId, HoldId, ReadingId, SlotId};
use portos_core::yard::{BlockSnapshot, YardBlock};
use portos_core::{CoreError, CoreResult};
use rusqlite::Row;

fn bad_row(col: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(0, col.to_string(), rusqlite::types::Type::Text)
}

// --------------------------------------------------------------------------
// Blocks & slots
// --------------------------------------------------------------------------

fn row_to_block(r: &Row<'_>) -> rusqlite::Result<YardBlock> {
    let id_s: String = r.get("id")?;
    Ok(YardBlock {
        id: BlockId::from_uuid(uuid::Uuid::parse_str(&id_s).map_err(|_| bad_row("id"))?),
        code: r.get("code")?,
        name_en: r.get("name_en")?,
        name_fa: r.get("name_fa")?,
        block_type: serde_json::from_str(&format!("\"{}\"", r.get::<_, String>("block_type")?))
            .map_err(|_| bad_row("block_type"))?,
        bays: r.get::<_, i64>("bays")? as u16,
        rows: r.get::<_, i64>("rows")? as u16,
        tiers: r.get::<_, i64>("tiers")? as u16,
        reefer_plugs: r.get::<_, i64>("reefer_plugs")? as u16,
        distance_to_quay_m: r.get::<_, i64>("distance_to_quay_m")? as u32,
        distance_to_gate_m: r.get::<_, i64>("distance_to_gate_m")? as u32,
    })
}

pub fn create_block(store: &Store, b: &YardBlock, pre_create_slots: bool) -> CoreResult<u32> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO yard_blocks (id, code, name_en, name_fa, block_type, bays, rows, tiers, reefer_plugs, distance_to_quay_m, distance_to_gate_m)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            rusqlite::params![
                b.id.to_string(), b.code, b.name_en, b.name_fa,
                crate::util::enum_str(&b.block_type),
                b.bays, b.rows, b.tiers, b.reefer_plugs,
                b.distance_to_quay_m, b.distance_to_gate_m
            ],
        )
        .map_err(map_err)?;
        let mut count = 0u32;
        if pre_create_slots {
            let mut stmt = c
                .prepare("INSERT INTO yard_slots (id, block_id, bay, row, tier, state) VALUES (?1,?2,?3,?4,?5,'free')")
                .map_err(map_err)?;
            for bay in 0..b.bays {
                for row in 0..b.rows {
                    for tier in 0..b.tiers {
                        let slot_id = SlotId::generate();
                        stmt.execute(rusqlite::params![
                            slot_id.to_string(), b.id.to_string(), bay, row, tier
                        ])
                        .map_err(map_err)?;
                        count += 1;
                    }
                }
            }
        }
        Ok(count)
    })
}

pub fn list_blocks(store: &Store) -> CoreResult<Vec<YardBlock>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c.prepare("SELECT * FROM yard_blocks ORDER BY code").map_err(map_err)?;
        let rows = stmt.query_map([], row_to_block).map_err(map_err)?;
        rows.filter_map(|r| r.ok()).map(Ok).collect()
    })
}

pub fn get_block(store: &Store, id: BlockId) -> CoreResult<YardBlock> {
    on_conn(&store.conn(), |c| {
        c.query_row("SELECT * FROM yard_blocks WHERE id = ?1", [id.to_string()], row_to_block)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!("block {id}")),
                _ => map_err(e),
            })
    })
}

/// One occupied slot as seen from the yard floor.
#[derive(Debug, Clone)]
pub struct OccupiedSlot {
    pub bay: u16,
    pub row: u16,
    pub tier: u16,
    pub container: ContainerNumber,
}

/// Occupied slots of a block (join to containers for IMDG classes).
pub fn occupied_slots(store: &Store, block: BlockId) -> CoreResult<Vec<OccupiedSlot>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare(
                "SELECT s.bay, s.row, s.tier, s.container_id
                 FROM yard_slots s WHERE s.block_id = ?1 AND s.state = 'occupied'",
            )
            .map_err(map_err)?;
        let rows = stmt
            .query_map([block.to_string()], |r| {
                let bay: i64 = r.get(0)?;
                let row_no: i64 = r.get(1)?;
                let tier: i64 = r.get(2)?;
                let cn: String = r.get(3)?;
                Ok((
                    bay as u16,
                    row_no as u16,
                    tier as u16,
                    ContainerNumber::parse(&cn).map_err(|_| bad_row("container_id"))?,
                ))
            })
            .map_err(map_err)?;
        let mut out = Vec::new();
        for r in rows {
            let (bay, row, tier, container) = r.map_err(map_err)?;
            out.push(OccupiedSlot { bay, row, tier, container });
        }
        Ok(out)
    })
}

/// Occupy a specific slot position; fails if already occupied.
pub fn occupy_slot(
    store: &Store,
    block: BlockId,
    bay: u16,
    row: u16,
    tier: u16,
    container: &ContainerNumber,
) -> CoreResult<SlotId> {
    on_conn(&store.conn(), |c| {
        let slot_id: String = c
            .query_row(
                "SELECT id FROM yard_slots WHERE block_id = ?1 AND bay = ?2 AND row = ?3 AND tier = ?4 AND state = 'free'",
                rusqlite::params![block.to_string(), bay, row, tier],
                |r| r.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => CoreError::Conflict(format!(
                    "no free slot at {block}/{bay}/{row}/{tier}"
                )),
                _ => map_err(e),
            })?;
        c.execute(
            "UPDATE yard_slots SET state = 'occupied', container_id = ?2 WHERE id = ?1 AND state = 'free'",
            rusqlite::params![slot_id, container.as_str()],
        )
        .map_err(map_err)?;
        Ok(SlotId::from_uuid(
            uuid::Uuid::parse_str(&slot_id).map_err(|e| CoreError::Internal(format!("slot uuid: {e}")))?,
        ))
    })
}

/// Free the slot holding a container (after gate-out / load).
pub fn release_container_slot(store: &Store, container: &ContainerNumber) -> CoreResult<bool> {
    on_conn(&store.conn(), |c| {
        let n = c
            .execute(
                "UPDATE yard_slots SET state = 'free', container_id = NULL WHERE container_id = ?1",
                [container.as_str()],
            )
            .map_err(map_err)?;
        Ok(n > 0)
    })
}

pub fn find_slot_of_container(store: &Store, container: &ContainerNumber) -> CoreResult<Option<(SlotId, BlockId, u16, u16, u16)>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare(
                "SELECT s.id, s.block_id, s.bay, s.row, s.tier FROM yard_slots s
                 JOIN yard_blocks b ON b.id = s.block_id
                 WHERE s.container_id = ?1",
            )
            .map_err(map_err)?;
        let mut rows = stmt
            .query_map([container.as_str()], |r| {
                let id_s: String = r.get(0)?;
                let block_s: String = r.get(1)?;
                Ok((
                    SlotId::from_uuid(uuid::Uuid::parse_str(&id_s).map_err(|_| bad_row("id"))?),
                    BlockId::from_uuid(uuid::Uuid::parse_str(&block_s).map_err(|_| bad_row("block_id"))?),
                    r.get::<_, i64>(2)? as u16,
                    r.get::<_, i64>(3)? as u16,
                    r.get::<_, i64>(4)? as u16,
                ))
            })
            .map_err(map_err)?;
        match rows.next() {
            Some(row) => Ok(Some(row.map_err(map_err)?)),
            None => Ok(None),
        }
    })
}

// --------------------------------------------------------------------------
// Containers
// --------------------------------------------------------------------------

fn row_to_container(r: &Row<'_>) -> rusqlite::Result<Container> {
    let customer: Option<String> = r.get("customer_id")?;
    Ok(Container {
        number: ContainerNumber::parse(&r.get::<_, String>("id")?).map_err(|_| bad_row("id"))?,
        size_type: IsoSizeType::parse(&r.get::<_, String>("size_type")?).map_err(|_| bad_row("size_type"))?,
        weight_kg: r.get::<_, i64>("weight_kg")? as u32,
        imdg_class: r.get("imdg_class")?,
        reefer_setpoint_c: r.get("reefer_setpoint_c")?,
        over_gauge: r.get::<_, i64>("over_gauge")? != 0,
        seal_number: r.get("seal_number")?,
        customer: customer
            .map(|s| CustomerId::from_uuid(uuid::Uuid::parse_str(&s).unwrap_or_default())),
        pol: r.get("pol")?,
        pod: r.get("pod")?,
    })
}

/// Insert or update a container master record (idempotent on number).
pub fn upsert_container(store: &Store, ct: &Container) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO containers (id, size_type, weight_kg, imdg_class, reefer_setpoint_c, over_gauge, seal_number,
                customer_id, pol, pod, state, customs_status, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?13)
             ON CONFLICT(id) DO UPDATE SET
                size_type = excluded.size_type,
                weight_kg = excluded.weight_kg,
                imdg_class = excluded.imdg_class,
                reefer_setpoint_c = excluded.reefer_setpoint_c,
                over_gauge = excluded.over_gauge,
                seal_number = excluded.seal_number,
                customer_id = excluded.customer_id,
                pol = excluded.pol,
                pod = excluded.pod,
                updated_at = excluded.updated_at",
            rusqlite::params![
                ct.number.as_str(), ct.size_type.as_str(), ct.weight_kg,
                ct.imdg_class, ct.reefer_setpoint_c, i64::from(ct.over_gauge),
                ct.seal_number, ct.customer.map(|c| c.to_string()), ct.pol, ct.pod,
                "advertised",
                "not_declared",
                ts(Utc::now())
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn get_container(store: &Store, number: &ContainerNumber) -> CoreResult<Container> {
    on_conn(&store.conn(), |c| {
        c.query_row("SELECT * FROM containers WHERE id = ?1", [number.as_str()], row_to_container)
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound(format!("container {number}")),
                _ => map_err(e),
            })
    })
}

pub fn list_containers(store: &Store, state: Option<ContainerState>, limit: u32) -> CoreResult<Vec<Container>> {
    on_conn(&store.conn(), |c| {
        let (sql, param): (&str, Option<String>) = match &state {
            Some(s) => (
                "SELECT * FROM containers WHERE state = ?1 ORDER BY updated_at DESC LIMIT ?2",
                Some(crate::util::enum_str(s)),
            ),
            None => ("SELECT * FROM containers ORDER BY updated_at DESC LIMIT ?1", None),
        };
        let mut stmt = c.prepare(sql).map_err(map_err)?;
        let rows = if let Some(p) = param {
            stmt.query_map(rusqlite::params![p, limit], row_to_container)
        } else {
            stmt.query_map([limit], row_to_container)
        }
        .map_err(map_err)?;
        rows.filter_map(|r| r.ok()).map(Ok).collect()
    })
}

pub fn set_container_state(store: &Store, number: &ContainerNumber, state: ContainerState) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "UPDATE containers SET state = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![
                number.as_str(),
                crate::util::enum_str(&state),
                ts(Utc::now())
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn set_customs_status(store: &Store, number: &ContainerNumber, status: CustomsStatus) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "UPDATE containers SET customs_status = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![
                number.as_str(),
                crate::util::enum_str(&status),
                ts(Utc::now())
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn mark_gate_in(store: &Store, number: &ContainerNumber, at: DateTime<Utc>) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "UPDATE containers SET gate_in_at = ?2, state = 'in_yard', updated_at = ?3 WHERE id = ?1",
            rusqlite::params![number.as_str(), ts(at), ts(Utc::now())],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

/// Stored gate-in timestamp of a container (starts storage billing — F-28).
pub fn gate_in_at(store: &Store, container: &ContainerNumber) -> CoreResult<Option<chrono::DateTime<chrono::Utc>>> {
    on_conn(&store.conn(), |c| {
        let raw: Option<String> = c
            .query_row("SELECT gate_in_at FROM containers WHERE id = ?1", [container.as_str()], |r| r.get(0))
            .map_err(map_err)?;
        parse_ts_opt(raw).map_err(|_| bad_row("gate_in_at")).map_err(map_err)
    })
}

pub fn mark_gate_out(store: &Store, number: &ContainerNumber, at: DateTime<Utc>) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "UPDATE containers SET gate_out_at = ?2, state = 'gate_out', updated_at = ?3 WHERE id = ?1",
            rusqlite::params![number.as_str(), ts(at), ts(Utc::now())],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

// --------------------------------------------------------------------------
// Holds (F-11)
// --------------------------------------------------------------------------

pub fn place_hold(store: &Store, hold: &Hold) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO holds (id, container_id, kind, reason, placed_by, placed_at, released_at, released_by)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            rusqlite::params![
                hold.id.to_string(), hold.container.as_str(),
                crate::util::enum_str(&hold.kind),
                hold.reason, hold.placed_by, ts(hold.placed_at),
                hold.released_at.map(ts), hold.released_by
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn active_holds(store: &Store, container: &ContainerNumber) -> CoreResult<Vec<Hold>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT * FROM holds WHERE container_id = ?1 AND released_at IS NULL ORDER BY placed_at")
            .map_err(map_err)?;
        let rows = stmt
            .query_map([container.as_str()], |r| {
                let kind: String = r.get("kind")?;
                Ok(Hold {
                    id: HoldId::from_uuid(
                        uuid::Uuid::parse_str(&r.get::<_, String>("id")?).map_err(|_| bad_row("id"))?,
                    ),
                    container: ContainerNumber::parse(&r.get::<_, String>("container_id")?)
                        .map_err(|_| bad_row("container_id"))?,
                    kind: serde_json::from_str(&format!("\"{kind}\"")).map_err(|_| bad_row("kind"))?,
                    reason: r.get("reason")?,
                    placed_by: r.get("placed_by")?,
                    placed_at: parse_ts(&r.get::<_, String>("placed_at")?).map_err(|_| bad_row("placed_at"))?,
                    released_at: parse_ts_opt(r.get("released_at")?).map_err(|_| bad_row("released_at"))?,
                    released_by: r.get("released_by")?,
                })
            })
            .map_err(map_err)?;
        rows.filter_map(|r| r.ok()).map(Ok).collect()
    })
}

pub fn release_hold(store: &Store, hold: HoldId, by: &str) -> CoreResult<bool> {
    on_conn(&store.conn(), |c| {
        let n = c
            .execute(
                "UPDATE holds SET released_at = ?2, released_by = ?3 WHERE id = ?1 AND released_at IS NULL",
                rusqlite::params![hold.to_string(), ts(Utc::now()), by],
            )
            .map_err(map_err)?;
        Ok(n > 0)
    })
}

// --------------------------------------------------------------------------
// Reefer readings (F-10)
// --------------------------------------------------------------------------

pub fn add_reefer_reading(
    store: &Store,
    container: &ContainerNumber,
    at: DateTime<Utc>,
    temperature_c: f64,
    power_ok: bool,
    alarm: Option<&str>,
) -> CoreResult<ReadingId> {
    let id = ReadingId::generate();
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO reefer_readings (id, container_id, at, temperature_c, power_ok, alarm)
             VALUES (?1,?2,?3,?4,?5,?6)",
            rusqlite::params![id.to_string(), container.as_str(), ts(at), temperature_c, i64::from(power_ok), alarm],
        )
        .map_err(map_err)?;
        Ok(())
    })?;
    Ok(id)
}

pub fn reefer_open_alarms(store: &Store, since: DateTime<Utc>) -> CoreResult<Vec<(ContainerNumber, DateTime<Utc>, f64, String)>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare(
                "SELECT container_id, at, temperature_c, alarm FROM reefer_readings
                 WHERE alarm IS NOT NULL AND at >= ?1 ORDER BY at DESC",
            )
            .map_err(map_err)?;
        let rows = stmt
            .query_map([ts(since)], |r| {
                Ok((
                    ContainerNumber::parse(&r.get::<_, String>(0)?).map_err(|_| bad_row("container_id"))?,
                    parse_ts(&r.get::<_, String>(1)?).map_err(|_| bad_row("at"))?,
                    r.get::<_, f64>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(map_err)?;
        rows.filter_map(|r| r.ok()).map(Ok).collect()
    })
}

// --------------------------------------------------------------------------
// Yard moves & snapshots
// --------------------------------------------------------------------------

pub fn record_yard_move(
    store: &Store,
    container: &ContainerNumber,
    from_slot: Option<SlotId>,
    to_slot: Option<SlotId>,
    reason: &str,
    by: &str,
    at: DateTime<Utc>,
) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO yard_moves (id, container_id, from_slot, to_slot, reason, at, by)
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![
                uuid::Uuid::now_v7().to_string(), container.as_str(),
                from_slot.map(|s| s.to_string()), to_slot.map(|s| s.to_string()),
                reason, ts(at), by
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

/// Build the planning snapshot of one block (occupied columns, IMDG
/// classes present, free plugs) for the slot assignment engine (F-7/F-8).
pub fn block_snapshot(store: &Store, block: &YardBlock) -> CoreResult<BlockSnapshot> {
    let occupied_rows = occupied_slots(store, block.id)?;
    let mut snapshot = BlockSnapshot {
        block: block.id,
        block_type: block.block_type,
        bays: block.bays,
        rows: block.rows,
        tiers: block.tiers,
        column_heights: Default::default(),
        free_plugs: block.reefer_plugs,
        imdg_classes_present: Vec::new(),
        occupied: 0,
        capacity: u32::from(block.bays * block.rows * block.tiers),
    };
    let mut heights: std::collections::HashMap<(u16, u16), u16> = Default::default();
    for slot in &occupied_rows {
        *heights.entry((slot.bay, slot.row)).or_insert(0) = heights
            .get(&(slot.bay, slot.row))
            .copied()
            .unwrap_or(0)
            .max(slot.tier + 1);
        snapshot.occupied += 1;
        if let Ok(ct) = get_container(store, &slot.container) {
            if let Some(class) = &ct.imdg_class {
                if !snapshot.imdg_classes_present.contains(class) {
                    snapshot.imdg_classes_present.push(class.clone());
                }
            }
            if ct.kind().needs_plug() && snapshot.free_plugs > 0 {
                snapshot.free_plugs -= 1;
            }
        }
    }
    snapshot.column_heights = heights;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use portos_core::container::{HoldKind, LengthClass};
    use portos_core::yard::BlockType;

    fn container(cn: &str, imdg: Option<&str>, reefer: bool) -> Container {
        Container {
            number: ContainerNumber::parse(cn).unwrap(),
            size_type: IsoSizeType::parse(if reefer { "45R1" } else { "22G1" }).unwrap(),
            weight_kg: 20_000,
            imdg_class: imdg.map(str::to_string),
            reefer_setpoint_c: if reefer { Some(-18.0) } else { None },
            over_gauge: false,
            seal_number: None,
            customer: None,
            pol: None,
            pod: None,
        }
    }

    #[test]
    fn block_slots_and_snapshots() {
        let store = Store::open_in_memory().unwrap();
        let block = YardBlock {
            id: BlockId::generate(),
            code: "A1".into(),
            name_en: "A1".into(),
            name_fa: "ا۱".into(),
            block_type: BlockType::General,
            bays: 3,
            rows: 2,
            tiers: 2,
            reefer_plugs: 0,
            distance_to_quay_m: 300,
            distance_to_gate_m: 500,
        };
        let n = create_block(&store, &block, true).unwrap();
        assert_eq!(n, 12);

        let ct = container("MSKU3881107", None, false);
        upsert_container(&store, &ct).unwrap();
        let slot = occupy_slot(&store, block.id, 0, 0, 0, &ct.number).unwrap();
        assert!(find_slot_of_container(&store, &ct.number).unwrap().is_some());

        let snap = block_snapshot(&store, &block).unwrap();
        assert_eq!(snap.occupied, 1);
        assert_eq!(*snap.column_heights.get(&(0, 0)).unwrap(), 1);

        // Occupying the same slot again must conflict.
        assert!(occupy_slot(&store, block.id, 0, 0, 0, &ct.number).is_err());
        // Stacking on top works.
        let ct2 = container("CSQU3054383", None, false);
        upsert_container(&store, &ct2).unwrap();
        occupy_slot(&store, block.id, 0, 0, 1, &ct2.number).unwrap();

        release_container_slot(&store, &ct2.number).unwrap();
        let snap = block_snapshot(&store, &block).unwrap();
        assert_eq!(snap.occupied, 1);

        set_container_state(&store, &ct.number, ContainerState::GateOut).unwrap();
        let listed = list_containers(&store, Some(ContainerState::GateOut), 100).unwrap();
        assert!(listed.iter().any(|c| c.number == ct.number));
        let _ = slot;
        let _ = LengthClass::Twenty;
    }

    #[test]
    fn holds_and_reefer() {
        let store = Store::open_in_memory().unwrap();
        let ct = container("TEMU1234565", None, false);
        upsert_container(&store, &ct).unwrap();
        let hold = Hold {
            id: HoldId::generate(),
            container: ct.number.clone(),
            kind: HoldKind::Customs,
            reason: "scan required".into(),
            placed_by: "customs".into(),
            placed_at: Utc::now(),
            released_at: None,
            released_by: None,
        };
        place_hold(&store, &hold).unwrap();
        assert_eq!(active_holds(&store, &ct.number).unwrap().len(), 1);
        release_hold(&store, hold.id, "customs").unwrap();
        assert!(active_holds(&store, &ct.number).unwrap().is_empty());

        set_customs_status(&store, &ct.number, CustomsStatus::Cleared).unwrap();
        let cleared: i64 = crate::util::on_conn(&store.conn(), |c| {
            c.query_row(
                "SELECT COUNT(*) FROM containers WHERE id = ?1 AND customs_status = 'cleared'",
                [ct.number.as_str()],
                |r| r.get(0),
            )
            .map_err(crate::util::map_err)
        })
        .unwrap();
        assert_eq!(cleared, 1);

        add_reefer_reading(&store, &ct.number, Utc::now(), -15.5, false, Some("temperature above setpoint")).unwrap();
        assert_eq!(reefer_open_alarms(&store, Utc::now() - chrono::Duration::hours(1)).unwrap().len(), 1);
    }
}
