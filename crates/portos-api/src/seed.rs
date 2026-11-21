//! Demo data seeder: realistic Phase-1 dataset for a container terminal.
//!
//! All entities are internally consistent — vessel IMO numbers and
//! container numbers are generated with valid check digits, and yard
//! placements go through the real segregation-aware assignment engine.
//! Seed users carry `Demo!`-style passwords that are Argon2-hashed on
//! insert; operators must rotate them in production.

use portos_core::berth::{Berth, CargoType};
use portos_core::container::{Container, ContainerNumber, ContainerState, CustomsStatus, Hold, HoldKind, IsoSizeType};
use portos_core::gate::{GateDirection, GateStepKind, GateTransaction, TruckAppointment, AppointmentStatus};
use portos_core::ids::{BlockId, CallId, CustomerId, HoldId, VesselId};
use portos_core::money::{Currency, FxEntry};
use portos_core::vessel::{CargoMix, CallStatus, ImoNumber, UnLocode, Vessel, VesselCall};
use portos_core::yard::BlockType;
use portos_core::CoreResult;
use portos_core::work::{Equipment, EquipmentKind, EquipmentStatus, Job, JobKind, JobStatus, MoveRecord};
use portos_core::billing::{InvoiceDraft, Tariff, TariffBasis};
use portos_core::ids::TariffId;
use portos_i18n::{Calendar, Locale, Numerals};
use portos_store::Store;

use crate::config::Config;

/// Small deterministic PRNG so seeded datasets are reproducible.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed ^ 0x9E3779B97F4A7C15)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo)
    }
}

pub fn seed_demo(store: &Store, _tenant: &str) -> CoreResult<String> {
    let mut rng = Rng::new(20261121);
    let now = chrono::Utc::now();
    let mut lines: Vec<String> = Vec::new();

    // ------------------------------------------------------------------
    // Users (argon2-hashed)
    // ------------------------------------------------------------------
    let demo_users: &[(&str, &str, &str, &str, Locale, Calendar, Numerals)] = &[
        ("admin", "Admin!2026", "مدیر سامانه / System Admin", "admin", Locale::Fa, Calendar::Jalali, Numerals::Persian),
        ("planner", "Planner!2026", "برنامه‌ریز عملیات بندری", "ops_planner", Locale::Fa, Calendar::Jalali, Numerals::Persian),
        ("yardplanner", "Yard!2026", "برنامه‌ریز محوطه", "yard_planner", Locale::Fa, Calendar::Jalali, Numerals::Persian),
        ("gateclerk", "Gate!2026", "کارمند گیت", "gate_clerk", Locale::Fa, Calendar::Jalali, Numerals::Persian),
        ("customs", "Customs!2026", "گمرک بندر", "customs", Locale::Fa, Calendar::Jalali, Numerals::Persian),
        ("finance", "Finance!2026", "واحد مالی", "finance", Locale::Fa, Calendar::Jalali, Numerals::Persian),
        ("agent", "Agent!2026", "نماینده کشتیرانی", "line_agent", Locale::En, Calendar::Gregorian, Numerals::Western),
    ];
    for (username, password, display, role, locale, calendar, numerals) in demo_users {
        if portos_store::users::find_by_username(store, username)?.is_none() {
            let hash = crate::auth::hash_password(password)?;
            portos_store::users::create(store, username, &hash, display, role, *locale, *calendar, *numerals)?;
        }
    }
    lines.push(format!("users: {}", demo_users.len()));

    // ------------------------------------------------------------------
    // Customers
    // ------------------------------------------------------------------
    let caspian = CustomerId::generate();
    let trucker = CustomerId::generate();
    portos_store::billing::create_customer(store, &portos_store::billing::CustomerRow {
        id: caspian,
        name_en: "Caspian Shipping Line".into(),
        name_fa: "کشتیرانی دریای خزر".into(),
        kind: "line".into(),
        credit_limit_minor: 500_000_000_000,
        currency: Currency::IRR,
        active: true,
    })?;
    portos_store::billing::create_customer(store, &portos_store::billing::CustomerRow {
        id: trucker,
        name_en: "Persian Road Logistics".into(),
        name_fa: "حمل و نقل جاده‌ای پارس".into(),
        kind: "trucker".into(),
        credit_limit_minor: 50_000_000_000,
        currency: Currency::IRR,
        active: true,
    })?;
    portos_store::billing::create_customer(store, &portos_store::billing::CustomerRow {
        id: CustomerId::generate(),
        name_en: "Bandar Abbas Customs".into(),
        name_fa: "گمرک بندرعباس".into(),
        kind: "customs".into(),
        credit_limit_minor: 0,
        currency: Currency::IRR,
        active: true,
    })?;
    lines.push("customers: 3".into());

    // ------------------------------------------------------------------
    // Vessels (valid IMO check digits) & berths & yard blocks
    // ------------------------------------------------------------------
    let vessel_specs: &[(&str, &str, u64, f64, f64, f64, u32, u16)] = &[
        ("MV Gulf Pearl", "گلف پرل", 912345, 294.0, 32.2, 14.0, 93_000, 600),
        ("MV Kharg Star", "ستاره خارک", 925671, 265.0, 32.0, 12.8, 68_000, 450),
        ("MV Caspian Meridian", "نصف‌النهار خزر", 938842, 199.9, 30.0, 11.5, 37_000, 300),
        ("MV Persian Dawn", "سپیده‌دم ایران", 945317, 181.0, 27.8, 10.4, 25_000, 250),
    ];
    let mut vessel_ids = Vec::new();
    for (name_en, name_fa, imo_prefix, loa, beam, draft, dwt, reefers) in vessel_specs {
        let imo = ImoNumber::from_prefix(&imo_prefix.to_string())?;
        let v = Vessel {
            id: VesselId::generate(),
            imo,
            name_en: (*name_en).into(),
            name_fa: (*name_fa).into(),
            mmsi: None,
            flag_code: Some("IR".into()),
            loa_m: *loa,
            beam_m: *beam,
            max_draft_m: *draft,
            dwt_t: *dwt,
            reefer_points: *reefers,
            gearless: true,
            customer: Some(caspian),
        };
        portos_store::fleet::create_vessel(store, &v)?;
        vessel_ids.push(v.id);
    }
    lines.push("vessels: 4 (valid IMO check digits)".into());

    let berth_specs: &[(&str, &str, f64, f64, u8, bool)] = &[
        ("B1", "اسکله ۱", 320.0, 15.0, 4, true),
        ("B2", "اسکله ۲", 280.0, 14.5, 4, true),
        ("B3", "اسکله ۳", 240.0, 13.0, 2, false),
        ("B4", "اسکله ۴", 200.0, 12.0, 2, false),
    ];
    for (code, name_fa, len, draught, cranes, shore) in berth_specs {
        portos_store::fleet::create_berth(store, &Berth {
            id: portos_core::ids::BerthId::generate(),
            code: (*code).into(),
            name_en: format!("Berth {}", code.trim_start_matches('B')),
            name_fa: (*name_fa).into(),
            length_m: *len,
            max_draught_m: *draught,
            crane_count: *cranes,
            shore_power: *shore,
            capabilities: vec![CargoType::Container],
            active: true,
        })?;
    }
    lines.push("berths: 4".into());

    let block_specs: &[(&str, &str, BlockType, u16, u16, u16, u16, u32, u32)] = &[
        ("A1", "بلوک الف۱", BlockType::General, 20, 6, 4, 0, 250, 600),
        ("A2", "بلوک الف۲", BlockType::General, 20, 6, 4, 0, 350, 500),
        ("RF1", "بلوک یخچالی ۱", BlockType::Reefer, 10, 6, 2, 120, 200, 700),
        ("DG1", "بلوک کالای خطرناک", BlockType::Dangerous, 10, 6, 2, 0, 900, 800),
        ("EM1", "بلوک خالی", BlockType::Empty, 10, 6, 3, 0, 400, 350),
        ("OOG1", "بلوک باری اضافه‌ارتفاع", BlockType::OverGauge, 6, 6, 2, 0, 300, 900),
    ];
    let mut slots_total = 0u32;
    for (code, name_fa, btype, bays, rows, tiers, plugs, dq, dg) in block_specs {
        let block = portos_core::yard::YardBlock {
            id: BlockId::generate(),
            code: (*code).into(),
            name_en: (*code).into(),
            name_fa: (*name_fa).into(),
            block_type: *btype,
            bays: *bays,
            rows: *rows,
            tiers: *tiers,
            reefer_plugs: *plugs,
            distance_to_quay_m: *dq,
            distance_to_gate_m: *dg,
        };
        let created = portos_store::yard::create_block(store, &block, true)?;
        slots_total += created;
    }
    lines.push(format!("yard blocks: 6, slots created: {slots_total}"));

    // ------------------------------------------------------------------
    // Tariffs & FX (configurable engine — F-27; VAT configurable per law)
    // ------------------------------------------------------------------
    let mk_tariff = |code: &str, en: &str, fa: &str, basis: TariffBasis, cur: Currency, unit: i64, free: u32, tiers: Vec<portos_core::billing::Tier>| Tariff {
        id: TariffId::generate(),
        code: code.into(),
        name_en: en.into(),
        name_fa: fa.into(),
        basis,
        currency: cur,
        unit_minor: unit,
        free_days: free,
        tiers,
        tax_rate: 0.09, // configurable VAT; adjust per applicable regulation
        customer: None,
        active: true,
    };
    portos_store::billing::upsert_tariff(store, &mk_tariff("STORAGE-IMP", "Import storage (per day)", "انبارش وارداتی (روزانه)", TariffBasis::PerDay, Currency::IRR, 850_000, 4, vec![
        portos_core::billing::Tier { up_to: Some(5.0), unit_minor: 850_000 },
        portos_core::billing::Tier { up_to: None, unit_minor: 1_200_000 },
    ]))?;
    portos_store::billing::upsert_tariff(store, &mk_tariff("MOVE-DIS", "Discharge handling (per move)", "تخلیه (هر جابجایی)", TariffBasis::PerMove, Currency::IRR, 1_450_000, 0, vec![]))?;
    portos_store::billing::upsert_tariff(store, &mk_tariff("MOVE-LOAD", "Load handling (per move)", "بارگیری (هر جابجایی)", TariffBasis::PerMove, Currency::IRR, 1_450_000, 0, vec![]))?;
    portos_store::billing::upsert_tariff(store, &mk_tariff("REEFER-PLUG", "Reefer plug service (per day)", "پریز یخچالی (روزانه)", TariffBasis::PerDay, Currency::IRR, 1_200_000, 0, vec![]))?;
    portos_store::billing::upsert_tariff(store, &mk_tariff("CALL-FEE", "Vessel call fee", "هزینه پذیرش شناور", TariffBasis::PerCall, Currency::IRR, 1_800_000_000, 0, vec![]))?;
    portos_store::billing::upsert_tariff(store, &mk_tariff("SHORE-KWH", "Shore power (per kWh)", "برق ساحلی (هر کیلووات‌ساعت)", TariffBasis::PerKwh, Currency::IRR, 45_000, 0, vec![]))?;
    portos_store::billing::upsert_tariff(store, &mk_tariff("DEM-LINE", "Line demurrage (per day, USD)", "دمِرِج خط کشتیرانی (روزانه)", TariffBasis::PerDay, Currency::USD, 7_500, 5, vec![]))?;
    lines.push("tariffs: 7 (storage, moves, reefer, call fee, shore power, demurrage)".into());

    // Demo FX rate; load production rates via /v1/billing/fx.
    portos_store::billing::upsert_fx(store, &FxEntry {
        base: Currency::USD,
        quote: Currency::IRR,
        rate: 100_000.0,
        as_of: now,
        source: "demo".into(),
    })?;
    portos_store::billing::upsert_fx(store, &FxEntry {
        base: Currency::EUR,
        quote: Currency::USD,
        rate: 1.08,
        as_of: now,
        source: "demo".into(),
    })?;
    lines.push("fx rates: 2 (demo values — replace via API)".into());

    // ------------------------------------------------------------------
    // Vessel calls: 2 berthed (KPI history), 1 anchored, 1 scheduled
    // ------------------------------------------------------------------
    let make_call = |vessel: VesselId, voyage: &str, eta_offset_h: i64, status: CallStatus, anchored: Option<i64>, berthed: Option<i64>, mix: CargoMix| VesselCall {
        id: CallId::generate(),
        vessel,
        voyage_code: voyage.into(),
        port: UnLocode::parse("IRBND").expect("IRBND"),
        eta_utc: now + chrono::Duration::hours(eta_offset_h),
        etd_utc: Some(now + chrono::Duration::hours(eta_offset_h + 20)),
        status,
        cargo_mix: mix,
        anchorage_arrived_at: anchored.map(|h| now + chrono::Duration::hours(h)),
        berthed_at: berthed.map(|h| now + chrono::Duration::hours(h)),
        work_completed_at: None,
        departed_at: None,
        created_by: "planner".into(),
        created_at: now - chrono::Duration::days(3),
        updated_at: now,
    };
    let mix = |dis: u32, load: u32, reefers: u16, dg: u16| CargoMix {
        discharge_teu: dis, load_teu: load, reefer_units: reefers, dangerous_units: dg, over_gauge_units: 0,
    };
    let call_berthed_1 = make_call(vessel_ids[0], "V212E", -30, CallStatus::Working, Some(-34), Some(-32), mix(2200, 900, 120, 8));
    let call_berthed_2 = make_call(vessel_ids[1], "V208W", -54, CallStatus::Completed, Some(-58), Some(-56), mix(1400, 1600, 40, 0));
    let call_anchored = make_call(vessel_ids[2], "V221E", -2, CallStatus::AnchorageArrived, Some(-1), None, mix(800, 400, 20, 4));
    let call_scheduled = make_call(vessel_ids[3], "V230N", 26, CallStatus::Scheduled, None, None, mix(600, 600, 10, 0));
    for c in [&call_berthed_1, &call_berthed_2, &call_anchored, &call_scheduled] {
        portos_store::fleet::create_call(store, c)?;
    }
    lines.push("vessel calls: 4 (2 working/completed, 1 at anchorage, 1 scheduled)".into());

    // Berth allocations for the two berthed calls (occupies the berths).
    let berths = portos_store::fleet::list_berths(store)?;
    for (i, call) in [&call_berthed_1, &call_berthed_2].iter().enumerate() {
        let berth = berths.get(i).expect("berths seeded");
        portos_store::fleet::create_allocation(store, &portos_core::berth::BerthAllocation {
            id: portos_core::ids::AllocationId::generate(),
            call: call.id,
            berth: berth.id,
            planned_from: call.berthed_at.unwrap_or(call.eta_utc),
            planned_to: call.etd_utc.unwrap_or_else(|| call.eta_utc + chrono::Duration::hours(20)),
            score: Some(0.8 + f64::from(i as u8) / 20.0),
            reasons: vec!["LOA fits berth length".into(), "cranes available".into()],
            decided_by: "planner".into(),
            decided_at: now - chrono::Duration::days(2),
        })?;
    }

    // ------------------------------------------------------------------
    // Containers: mix of general, reefer, DG, OOG, empties (valid ISO 6346)
    // ------------------------------------------------------------------
    let prefixes = ["CSQU", "MSKU", "TEMU", "HLXU", "TRTU", "CSPU"];
    let mut containers = Vec::new();
    for i in 0..44usize {
        let prefix = prefixes[i % prefixes.len()];
        let serial = format!("{:06}", 100_000 + i * 137 + (rng.next() % 90) as usize);
        let cn = ContainerNumber::from_prefix(&format!("{prefix}{serial}"))?;
        let role = match i % 11 {
            0 | 1 => ("reefer", "45R1"),
            2 => ("dg", "22G1"),
            3 => ("oog", "42P1"),
            4 | 5 => ("empty", "22G1"),
            _ => ("general", if i % 2 == 0 { "22G1" } else { "45G1" }),
        };
        let ct = Container {
            number: cn,
            size_type: IsoSizeType::parse(role.1)?,
            weight_kg: match role.0 {
                "empty" => 0,
                _ => rng.range(8_000, 27_000) as u32,
            },
            imdg_class: match role.0 {
                "dg" => Some(if i % 22 == 2 { "3".into() } else { "8".into() }),
                _ => None,
            },
            reefer_setpoint_c: match role.0 {
                "reefer" => Some(-18.0),
                _ => None,
            },
            over_gauge: role.0 == "oog",
            seal_number: Some(format!("SL-{}", 100_000 + i)),
            customer: Some(caspian),
            pol: Some("IRBND".into()),
            pod: Some(if i % 3 == 0 { "AEJEA".into() } else { "CNSHA".into() }),
        };
        portos_store::yard::upsert_container(store, &ct)?;
        containers.push(ct);
    }
    lines.push(format!("containers registered: {} (valid ISO 6346 check digits)", containers.len()));

    // ------------------------------------------------------------------
    // Place containers through the real assignment engine
    // ------------------------------------------------------------------
    let blocks = portos_store::yard::list_blocks(store)?;
    let meta: std::collections::HashMap<BlockId, &portos_core::yard::YardBlock> =
        blocks.iter().map(|b| (b.id, b)).collect();
    let mut placed = 0usize;
    let mut placed_numbers = Vec::new();
    for ct in &containers {
        let block_type_needed = if ct.over_gauge {
            portos_core::yard::BlockType::OverGauge
        } else if ct.imdg_class.is_some() {
            portos_core::yard::BlockType::Dangerous
        } else if ct.kind().needs_plug() {
            portos_core::yard::BlockType::Reefer
        } else {
            portos_core::yard::BlockType::General
        };
        let mut snapshots = Vec::new();
        for b in &blocks {
            snapshots.push(portos_store::yard::block_snapshot(store, b)?);
        }
        let snap_refs: Vec<&portos_core::yard::BlockSnapshot> = snapshots.iter().collect();
        match portos_core::yard::suggest_slot(ct, &snap_refs, &meta, portos_core::yard::NextLeg::Vessel, &portos_core::yard::AssignmentPolicy::default()) {
            Ok(suggestion) => {
                if let Some(slot) = suggestion.slot {
                    portos_store::yard::occupy_slot(store, slot.block, slot.bay, slot.row, slot.tier, &ct.number)?;
                    portos_store::yard::record_yard_move(store, &ct.number, None, Some(slot.id), "seed: initial placement", "system:seed", now)?;
                    portos_store::yard::set_container_state(store, &ct.number, ContainerState::InYard)?;
                    portos_store::yard::set_customs_status(store, &ct.number, if ct.number.as_str().ends_with('3') { CustomsStatus::Cleared } else { CustomsStatus::Declared })?;
                    // gate-in a few hours/days ago → drives storage billing + dwell.
                    let gate_in = now - chrono::Duration::hours(rng.range(6, 240) as i64);
                    portos_store::yard::mark_gate_in(store, &ct.number, gate_in)?;
                    placed += 1;
                    placed_numbers.push(ct.number.clone());
                }
            }
            Err(_) => continue, // yard full for that type; keep advertised
        }
        let _ = block_type_needed;
    }
    lines.push(format!("containers placed in yard via assignment engine: {placed}"));

    // One customs hold on a DG box (F-11).
    if let Some(dg) = containers.iter().find(|c| c.imdg_class.is_some()) {
        portos_store::yard::place_hold(store, &Hold {
            id: HoldId::generate(),
            container: dg.number.clone(),
            kind: HoldKind::Customs,
            reason: "document verification pending".into(),
            placed_by: "customs".into(),
            placed_at: now - chrono::Duration::hours(5),
            released_at: None,
            released_by: None,
        })?;
        lines.push("holds: 1 (customs, on a DG container)".into());
    }

    // ------------------------------------------------------------------
    // Equipment, jobs and moves (gross crane productivity — F-32)
    // ------------------------------------------------------------------
    let mut crane_ids = Vec::new();
    for i in 1..=4u32 {
        let eq = Equipment {
            id: portos_core::ids::EquipmentId::generate(),
            code: format!("QC-{i:02}"),
            kind: EquipmentKind::QuayCrane,
            status: EquipmentStatus::Available,
            hours_meter: 8_000.0 + rng.range(0, 4000) as f64,
            last_service_at: Some(now - chrono::Duration::days(20)),
            telemetry: Some(serde_json::json!({"cycles_today": rng.range(80, 240)})),
        };
        portos_store::work::upsert_equipment(store, &eq)?;
        crane_ids.push(eq.id);
    }
    for i in 1..=6u32 {
        portos_store::work::upsert_equipment(store, &Equipment {
            id: portos_core::ids::EquipmentId::generate(),
            code: format!("SC-{i:03}"),
            kind: EquipmentKind::StraddleCarrier,
            status: EquipmentStatus::Available,
            hours_meter: 4_000.0 + rng.range(0, 2000) as f64,
            last_service_at: None,
            telemetry: None,
        })?;
    }
    lines.push("equipment: 4 quay cranes + 6 straddle carriers".into());

    let mut moves = 0u32;
    let mut failures = 0u32;
    for call in [&call_berthed_1, &call_berthed_2] {
        // ~30 moves over the last day at ~2.5 min gross cadence → ~24 moves/h.
        let base = call.berthed_at.unwrap_or(now - chrono::Duration::hours(20));
        for i in 0..60u32 {
            let started = base + chrono::Duration::minutes(i64::from(i) * 3);
            let success = rng.next() % 25 != 0; // ~4% failed moves
            if !success {
                failures += 1;
            }
            let job = Job {
                id: portos_core::ids::JobId::generate(),
                kind: if i % 3 == 0 { JobKind::Load } else { JobKind::Discharge },
                call: Some(call.id),
                container: placed_numbers.get(i as usize % placed_numbers.len().max(1)).cloned(),
                assigned_to: Some(crane_ids[(i % 4) as usize]),
                priority: 5,
                status: JobStatus::Done,
                created_at: started - chrono::Duration::minutes(2),
                assigned_at: Some(started - chrono::Duration::minutes(1)),
                completed_at: Some(started + chrono::Duration::seconds(150)),
                issue: None,
            };
            portos_store::work::create_job(store, &job)?;
            if let Some(cn) = &job.container {
                portos_store::work::record_move(store, &MoveRecord {
                    id: portos_core::ids::MoveId::generate(),
                    job: job.id,
                    equipment: job.assigned_to.unwrap(),
                    container: cn.clone(),
                    kind: job.kind,
                    started_at: started,
                    finished_at: started + chrono::Duration::seconds(150),
                    success,
                })?;
                moves += 1;
            }
        }
    }
    lines.push(format!("jobs+moves: {moves} confirmed moves across 2 calls ({failures} failed)"));

    // ------------------------------------------------------------------
    // Truck appointments + a completed gate transaction (F-17..F-19)
    // ------------------------------------------------------------------
    let mut appts = 0;
    for i in 0..6usize {
        let slot_from = now + chrono::Duration::hours(i64::from(i as u32 % 3) - 1);
        let appt = TruckAppointment {
            id: portos_core::ids::AppointmentId::generate(),
            reference: format!("TA-{}-{:06}", now.format("%Y"), i + 1),
            trucking_company: Some(trucker),
            driver_name: if i % 2 == 0 { "رضا محمدی".into() } else { "Hossein Karimi".into() },
            driver_license: format!("DL-{:06}", 77_000 + i),
            plate: format!("{}A{:05}", 10 + i, 12_300 + i * 7),
            container: placed_numbers.get(i).cloned(),
            direction: if i % 2 == 0 { GateDirection::Export } else { GateDirection::Import },
            slot_from,
            slot_to: slot_from + chrono::Duration::hours(1),
            status: if i < 2 { AppointmentStatus::CheckedIn } else { AppointmentStatus::Booked },
            created_at: now - chrono::Duration::hours(2),
        };
        portos_store::gate::create_appointment(store, &appt)?;
        appts += 1;
    }
    lines.push(format!("truck appointments: {appts}"));

    // One fully completed export gate-in with pass + EIR.
    if let Some(cn) = placed_numbers.first().cloned() {
        let mut tx = GateTransaction::start(
            GateDirection::Export,
            None,
            Some(cn.clone()),
            "12A34567",
            "DL-077000",
            "L1",
            now - chrono::Duration::minutes(30),
        );
        for kind in [
            GateStepKind::AppointmentCheck,
            GateStepKind::DriverId,
            GateStepKind::OcrScan,
            GateStepKind::SealCheck,
            GateStepKind::ReleaseCheck,
            GateStepKind::YardHandover,
        ] {
            if kind == GateStepKind::OcrScan {
                tx.set_ocr(Some(cn.clone()));
            }
            tx.apply(kind, true, None, tx.started_at + chrono::Duration::seconds(4))?;
        }
        portos_store::gate::create_gate_tx(store, &tx, None)?;
        portos_store::gate::save_gate_pass(store, &portos_core::gate::GatePass::issue(&tx, 1))?;
        portos_store::gate::save_eir(store, &portos_core::gate::EquipmentInterchangeReport::create(&tx, cn, 1))?;
        lines.push("gate: 1 completed export transaction with gate pass + EIR".into());
    }

    // ------------------------------------------------------------------
    // Sample invoice from the billing engine (storage for the oldest box)
    // ------------------------------------------------------------------
    if let Some(oldest) = containers.first() {
        let gate_in = portos_store::yard::gate_in_at(store, &oldest.number)?;
        if let Some(gate_in) = gate_in {
            let tariffs = portos_store::billing::list_tariffs(store, Some(caspian))?;
            if let Some(storage) = tariffs.iter().find(|t| t.code == "STORAGE-IMP") {
                let charge = portos_core::billing::compute_day_charge(storage, gate_in, None, now)?;
                let line = portos_core::billing::ChargeLine {
                    tariff_code: storage.code.clone(),
                    description_en: format!("Yard storage ({} day(s), {} free)", charge.days_billed, charge.free_days),
                    description_fa: format!("انبارش محوطه ({} روز، {} روز رایگان)", charge.days_billed, charge.free_days),
                    charge_ref: portos_core::billing::ChargeRef::Container { container: oldest.number.clone() },
                    quantity: charge.days_billed as f64,
                    unit_minor: storage.unit_minor,
                    amount: charge.amount,
                    tax_rate: storage.tax_rate,
                    tax_minor: (charge.amount.minor as f64 * storage.tax_rate).round() as i64,
                };
                let mut draft = InvoiceDraft::new(caspian, Currency::IRR);
                if line.amount.minor > 0 {
                    draft.push(line)?;
                }
                if draft.lines.is_empty() {
                    draft.push(portos_core::billing::ChargeLine {
                        tariff_code: "MOVE-DIS".into(),
                        description_en: "Handling moves (1 move(s))".into(),
                        description_fa: "جابجایی‌های بارگیری و تخلیه (۱ جابجایی)".into(),
                        charge_ref: portos_core::billing::ChargeRef::Unspecified { label: "demo".into() },
                        quantity: 1.0,
                        unit_minor: 1_450_000,
                        amount: portos_core::money::Money::new(1_450_000, Currency::IRR),
                        tax_rate: 0.09,
                        tax_minor: 130_500,
                    })?;
                }
                let seq = portos_store::billing::next_invoice_seq(store)? + 1;
                let lines_payload = draft.lines.clone();
                let invoice = draft.finalize(format!("INV-{}-{seq:06}", now.format("%Y")), now, 30);
                portos_store::billing::create_invoice(store, &invoice, &lines_payload)?;
                lines.push(format!("invoices: 1 ({} — storage days billed {})", invoice.number, charge.days_billed));
            }
        }
    }

    // One open safety incident (F-38).
    portos_store::misc::create_incident(store, &portos_store::misc::IncidentRow {
        id: portos_core::ids::IncidentId::generate(),
        kind: "near_miss".into(),
        severity: "minor".into(),
        location: Some("Yard A1".into()),
        description: "Truck brake failure near pedestrian walkway; no injuries".into(),
        occurred_at: now - chrono::Duration::hours(3),
        reported_by: "gateclerk".into(),
        status: "open".into(),
        actions: vec!["walkway temporarily rerouted".into()],
    })?;
    lines.push("safety incidents: 1 open near-miss".into());

    Ok(lines.join("\n"))
}
