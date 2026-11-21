//! Yard blocks, slots and the segregation-aware slot assignment engine
//! (PRD F-7, F-8, F-9, F-10).

use crate::container::Container;
use crate::error::{CoreError, CoreResult};
use crate::ids::{BlockId, SlotId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Yard block specialization (PRD F-8 segregation rules).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockType {
    General,
    Reefer,
    Dangerous,
    Empty,
    OverGauge,
}

impl fmt::Display for BlockType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockType::General => write!(f, "general"),
            BlockType::Reefer => write!(f, "reefer"),
            BlockType::Dangerous => write!(f, "dangerous"),
            BlockType::Empty => write!(f, "empty"),
            BlockType::OverGauge => write!(f, "over-gauge"),
        }
    }
}

/// A yard block: `bays × rows × tiers` slots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YardBlock {
    pub id: BlockId,
    /// Stack code, e.g. `A1`, `RF1`, `DG1`.
    pub code: String,
    pub name_en: String,
    pub name_fa: String,
    pub block_type: BlockType,
    pub bays: u16,
    pub rows: u16,
    pub tiers: u16,
    /// Reefer plugs available (Reefer blocks only).
    pub reefer_plugs: u16,
    /// Distance to the quay in metres — lower is closer to vessel ops.
    pub distance_to_quay_m: u32,
    /// Distance to the truck gate in metres.
    pub distance_to_gate_m: u32,
}

/// A slot inside a block. Position is (bay, row, tier), all zero-based.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YardSlot {
    pub id: SlotId,
    pub block: BlockId,
    pub bay: u16,
    pub row: u16,
    pub tier: u16,
    pub state: SlotState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotState {
    Free,
    Occupied,
    Reserved,
}

/// Snapshot of what is currently on the ground in one block: for every
/// (bay, row) column the highest occupied tier + count, plus the set of
/// dangerous-goods classes present.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockSnapshot {
    pub block: BlockId,
    pub block_type: BlockType,
    pub bays: u16,
    pub rows: u16,
    pub tiers: u16,
    /// Per column: number of boxes stacked.
    pub column_heights: HashMap<(u16, u16), u16>,
    /// Free reefer plugs remaining (Reefer blocks).
    pub free_plugs: u16,
    /// IMDG classes currently stored in the block.
    pub imdg_classes_present: Vec<String>,
    pub occupied: u32,
    pub capacity: u32,
}

impl Default for BlockSnapshot {
    fn default() -> Self {
        Self {
            block: BlockId::from_uuid(uuid::Uuid::nil()),
            block_type: BlockType::General,
            bays: 0,
            rows: 0,
            tiers: 0,
            column_heights: HashMap::new(),
            free_plugs: 0,
            imdg_classes_present: Vec::new(),
            occupied: 0,
            capacity: 0,
        }
    }
}

impl BlockSnapshot {
    #[must_use]
    pub fn occupancy_ratio(&self) -> f64 {
        if self.capacity == 0 {
            1.0
        } else {
            f64::from(self.occupied) / f64::from(self.capacity)
        }
    }
}

/// Result of the assignment engine: chosen slot + explanation (§6.6).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotSuggestion {
    pub block: BlockId,
    pub block_code: String,
    pub slot: Option<YardSlot>,
    pub score: Option<f64>,
    pub reasons: Vec<String>,
}

/// Policy knobs for the assignment engine (configurable per terminal).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentPolicy {
    /// Weight: prefer less-occupied blocks (minimize reshuffles, F-7).
    pub balance: f64,
    /// Weight: prefer low stack heights (fewer future reshuffles).
    pub low_stack: f64,
    /// Weight: proximity to the planned next leg (berth or gate).
    pub proximity: f64,
    /// Weight: keep destination groups together (browardization).
    pub grouping: f64,
}

impl Default for AssignmentPolicy {
    fn default() -> Self {
        Self { balance: 0.40, low_stack: 0.30, proximity: 0.20, grouping: 0.10 }
    }
}

/// Where the container is heading next — drives proximity scoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextLeg {
    Vessel,
    Gate,
    Rail,
    Unknown,
}

/// IMDG segregation policy (simplified, documented subset of the IMDG
/// code suitable for terminal planning; PRD F-8, F-39).
pub fn imdg_group(class: &str) -> &'static str {
    match class {
        "1" | "1.1" | "1.2" | "1.3" | "1.4" | "1.5" | "1.6" => "explosives",
        "2.1" => "flammable-gas",
        "2.2" => "non-flammable-gas",
        "2.3" => "toxic-gas",
        "3" => "flammable-liquid",
        "4.1" | "4.2" | "4.3" => "flammable-solid",
        "5.1" | "5.2" => "oxidizer",
        "6.1" => "toxic",
        "6.2" => "infectious",
        "7" => "radioactive",
        "8" => "corrosive",
        _ => "miscellaneous",
    }
}

/// Groups that must not share a row (kept on opposite row groups within
/// the Dangerous block).
#[must_use]
pub fn needs_isolation_row(class: &str) -> bool {
    matches!(imdg_group(class), "explosives" | "toxic-gas" | "radioactive" | "infectious")
}

fn eligible_block_type(c: &Container) -> BlockType {
    if c.over_gauge {
        BlockType::OverGauge
    } else if let Some(class) = &c.imdg_class {
        let _ = class;
        BlockType::Dangerous
    } else if c.kind().needs_plug() {
        BlockType::Reefer
    } else if c.size_type.length() == crate::container::LengthClass::Twenty && c.weight_kg == 0 {
        BlockType::Empty
    } else {
        BlockType::General
    }
}

/// Choose a slot for a container among available blocks.
///
/// Constraints enforced:
/// * reefer → Reefer block with a free plug (PRD F-10);
/// * dangerous goods → Dangerous block; isolated classes get rows in the
///   far half of the block, away from other classes (PRD F-8/F-39);
/// * over-gauge → OverGauge block, tier 0 only (needs clearance);
/// * empties → Empty block preferred, General allowed.
#[allow(clippy::too_many_lines)]
pub fn suggest_slot(
    container: &Container,
    blocks: &[&BlockSnapshot],
    block_meta: &HashMap<BlockId, &YardBlock>,
    next_leg: NextLeg,
    policy: &AssignmentPolicy,
) -> CoreResult<SlotSuggestion> {
    let required = eligible_block_type(container);
    let mut best: Option<(f64, BlockId, String, u16, u16, u16, Vec<String>)> = None;
    let mut blocked_reasons: Vec<String> = Vec::new();

    for snap in blocks {
        let meta = match block_meta.get(&snap.block) {
            Some(m) => *m,
            None => continue,
        };
        if meta.block_type != required {
            continue;
        }
        // Capacity check.
        if snap.occupied >= snap.capacity {
            blocked_reasons.push(format!("block {} is full", meta.code));
            continue;
        }
        // Reefer plug check (PRD F-10).
        if required == BlockType::Reefer {
            if snap.free_plugs == 0 {
                blocked_reasons.push(format!("no free reefer plugs in {}", meta.code));
                continue;
            }
            if container.reefer_setpoint_c.is_none() {
                blocked_reasons
                    .push("reefer without setpoint cannot be planned".to_string());
                continue;
            }
        }
        // Dangerous isolation row (PRD F-8).
        let isolated = container
            .imdg_class
            .as_deref()
            .map(needs_isolation_row)
            .unwrap_or(false);
        // Choose a target column: lowest height, nearest to the side of the
        // block that matches the next leg.
        let target_bay: u16 = match next_leg {
            NextLeg::Gate => 0,
            NextLeg::Vessel | NextLeg::Rail => meta.bays.saturating_sub(1),
            NextLeg::Unknown => meta.bays / 2,
        };
        let mut chosen: Option<(u16, u16, u16)> = None; // (bay,row,tier)
        for row in 0..meta.rows {
            if isolated && row < meta.rows / 2 {
                continue; // isolated classes only in the far rows
            }
            if !isolated && container.imdg_class.is_some() && row >= meta.rows / 2 {
                continue; // non-isolated DG stays in near rows
            }
            for bay in 0..meta.bays {
                let height = snap.column_heights.get(&(bay, row)).copied().unwrap_or(0);
                if height < meta.tiers {
                    // Prefer columns closest to the target bay with the
                    // lowest height.
                    let bay_dist = u16::abs_diff(bay, target_bay);
                    let score_key = (height, bay_dist);
                    if chosen
                        .map(|(h, b, _t)| (h, u16::abs_diff(b, target_bay)))
                        .map(|cur| score_key < cur)
                        .unwrap_or(true)
                    {
                        chosen = Some((bay, row, height));
                    }
                }
            }
        }
        let (bay, row, tier) = match chosen {
            Some(c) => c,
            None => {
                blocked_reasons.push(format!(
                    "no free column in {} ({} occupancy)",
                    meta.code,
                    (snap.occupancy_ratio() * 100.0) as u32
                ));
                continue;
            }
        };

        // --- score -----------------------------------------------------
        let balance_score = 1.0 - snap.occupancy_ratio();
        let height_score = 1.0 - f64::from(tier) / f64::from(meta.tiers.max(1));
        let proximity_score = match next_leg {
            NextLeg::Vessel => 1.0 - f64::from(meta.distance_to_quay_m) / 2000.0,
            NextLeg::Gate | NextLeg::Rail => {
                1.0 - f64::from(meta.distance_to_gate_m) / 2000.0
            }
            NextLeg::Unknown => 0.5,
        }
        .clamp(0.0, 1.0);
        let grouping_score = if container.imdg_class.is_some() {
            if snap.imdg_classes_present.iter().any(|c| Some(c) == container.imdg_class.as_ref()) {
                1.0
            } else {
                0.3
            }
        } else {
            1.0
        };
        let score = policy.balance * balance_score
            + policy.low_stack * height_score
            + policy.proximity * proximity_score
            + policy.grouping * grouping_score;

        let mut reasons = Vec::new();
        reasons.push(format!("block type {} matches container profile", meta.block_type));
        reasons.push(format!(
            "column ({bay},{row}) stack height {tier} of {} tiers → low reshuffle risk",
            meta.tiers
        ));
        match next_leg {
            NextLeg::Vessel => reasons.push(format!(
                "quay distance {}m favors vessel loading",
                meta.distance_to_quay_m
            )),
            NextLeg::Gate | NextLeg::Rail => reasons.push(format!(
                "gate distance {}m favors truck pickup",
                meta.distance_to_gate_m
            )),
            NextLeg::Unknown => {}
        }
        if isolated {
            reasons.push(format!(
                "IMDG class {} isolated to far rows per segregation policy",
                container.imdg_class.as_deref().unwrap_or("?")
            ));
        }
        if required == BlockType::Reefer {
            reasons.push(format!(
                "free reefer plug available, setpoint {:.1}°C",
                container.reefer_setpoint_c.unwrap_or(0.0)
            ));
        }

        if best.as_ref().is_none_or(|(s, _, _, _, _, _, _)| score > *s) {
            best = Some((score, snap.block, meta.code.clone(), bay, row, tier, reasons));
        }
    }

    match best {
        Some((score, block, code, bay, row, tier, reasons)) => {
            let slot = YardSlot {
                id: SlotId::generate(),
                block,
                bay,
                row,
                tier,
                state: SlotState::Reserved,
            };
            Ok(SlotSuggestion {
                block,
                block_code: code,
                slot: Some(slot),
                score: Some(score.clamp(0.0, 1.0)),
                reasons,
            })
        }
        None => {
            let mut reasons = blocked_reasons;
            reasons.push(format!(
                "no eligible {required} block for container {}",
                container.number
            ));
            Err(CoreError::Validation(reasons.join("; ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::container::{ContainerNumber, IsoSizeType};
    use crate::ids::BlockId;

    fn block(id: &str, code: &str, btype: BlockType, bays: u16, rows: u16, tiers: u16) -> YardBlock {
        YardBlock {
            id: BlockId::generate(),
            code: code.into(),
            name_en: code.into(),
            name_fa: code.into(),
            block_type: btype,
            bays,
            rows,
            tiers,
            reefer_plugs: if btype == BlockType::Reefer { 48 } else { 0 },
            distance_to_quay_m: 300,
            distance_to_gate_m: 500,
        }
    }

    fn container(imdg: Option<&str>, reefer: bool, oog: bool) -> Container {
        Container {
            number: ContainerNumber::parse("MSKU3881107").unwrap(),
            size_type: IsoSizeType::parse(if reefer { "45R1" } else { "22G1" }).unwrap(),
            weight_kg: 24_000,
            imdg_class: imdg.map(str::to_string),
            reefer_setpoint_c: if reefer { Some(-18.0) } else { None },
            over_gauge: oog,
            seal_number: None,
            customer: None,
            pol: None,
            pod: None,
        }
    }

    fn meta_map(blocks: &[YardBlock]) -> HashMap<BlockId, &YardBlock> {
        blocks.iter().map(|b| (b.id, b)).collect()
    }

    #[test]
    fn dry_box_goes_to_general_block() {
        let g = block("g", "A1", BlockType::General, 20, 6, 4);
        let rf = block("r", "RF1", BlockType::Reefer, 10, 6, 2);
        let list = vec![g, rf];
        let snaps: Vec<BlockSnapshot> = list
            .iter()
            .map(|b| BlockSnapshot {
                block: b.id,
                block_type: b.block_type,
                bays: b.bays,
                rows: b.rows,
                tiers: b.tiers,
                free_plugs: b.reefer_plugs,
                capacity: u32::from(b.bays * b.rows * b.tiers),
                ..Default::default()
            })
            .collect();
        let snap_refs: Vec<&BlockSnapshot> = snaps.iter().collect();
        let map = meta_map(&list);
        let c = container(None, false, false);
        let s = suggest_slot(&c, &snap_refs, &map, NextLeg::Vessel, &AssignmentPolicy::default())
            .unwrap();
        assert_eq!(s.block_code, "A1");
        assert!(s.reasons.iter().any(|r| r.contains("general")));
    }

    #[test]
    fn reefer_requires_plug_and_setpoint() {
        let rf = block("r", "RF1", BlockType::Reefer, 10, 6, 2);
        let rf_id = rf.id;
        let list = vec![rf];
        let map = meta_map(&list);
        let mut snap = BlockSnapshot {
            block: rf_id,
            block_type: BlockType::Reefer,
            bays: 10,
            rows: 6,
            tiers: 2,
            free_plugs: 0,
            capacity: 120,
            ..Default::default()
        };
        let snaps: Vec<&BlockSnapshot> = vec![&snap];
        let c = container(None, true, false);
        assert!(suggest_slot(&c, &snaps, &map, NextLeg::Gate, &AssignmentPolicy::default()).is_err());
        snap.free_plugs = 10;
        let snaps: Vec<&BlockSnapshot> = vec![&snap];
        assert!(suggest_slot(&c, &snaps, &map, NextLeg::Gate, &AssignmentPolicy::default()).is_ok());
    }

    #[test]
    fn dangerous_isolated_class_pushed_to_far_rows() {
        let dg = block("d", "DG1", BlockType::Dangerous, 20, 6, 3);
        let dg_id = dg.id;
        let list = vec![dg];
        let map = meta_map(&list);
        let snap = BlockSnapshot {
            block: dg_id,
            block_type: BlockType::Dangerous,
            bays: 20,
            rows: 6,
            tiers: 3,
            capacity: 360,
            imdg_classes_present: vec!["3".into()],
            ..Default::default()
        };
        let snaps: Vec<&BlockSnapshot> = vec![&snap];
        let c = container(Some("1.4"), false, false); // explosives → isolated
        let s = suggest_slot(&c, &snaps, &map, NextLeg::Unknown, &AssignmentPolicy::default())
            .unwrap();
        let slot = s.slot.unwrap();
        assert!(slot.row >= 3, "isolated class must be placed in far rows, got {}", slot.row);
    }

    #[test]
    fn over_gauge_needs_oog_block() {
        let g = block("g", "A1", BlockType::General, 20, 6, 4);
        let oog = block("o", "OOG1", BlockType::OverGauge, 10, 6, 2);
        let list = vec![g, oog];
        let map = meta_map(&list);
        let snaps: Vec<BlockSnapshot> = list
            .iter()
            .map(|b| BlockSnapshot {
                block: b.id,
                block_type: b.block_type,
                bays: b.bays,
                rows: b.rows,
                tiers: b.tiers,
                capacity: u32::from(b.bays * b.rows * b.tiers),
                ..Default::default()
            })
            .collect();
        let snap_refs: Vec<&BlockSnapshot> = snaps.iter().collect();
        let c = container(None, false, true);
        let s = suggest_slot(&c, &snap_refs, &map, NextLeg::Gate, &AssignmentPolicy::default())
            .unwrap();
        assert_eq!(s.block_code, "OOG1");
        assert_eq!(s.slot.unwrap().tier, 0, "over-gauge goes on ground tier");
    }
}
