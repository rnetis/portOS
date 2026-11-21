//! BAPLIE — stowage plan (PRD F-6 / I-2).
//!
//! Supported subset (BAPLIE 2.x style):
//! ```text
//! UNH+REF+BAPLIE:D:96A:UN'
//! TDT+20+V212E+++SEA:172:20+GULF PEARL:146:20'
//! LOC+5+IRBND'                       (port of call)
//! EQD+CN+MSKU3881107+22G1+++5'
//! MEA+WT++KGM:24000'
//! LOC+147+0000102:0000401:0000502'   (bay:row:tier, 7-digit zero-padded)
//! UNT+7+REF'
//! ```
//! Positions use the standard bay/row/tier encoding (`0000102` = bay 001,
//! row 02 of 10 pair, `0000502` = tier 50.2). We parse the numeric values.

use crate::dt;
use crate::syntax::{envelope, EdiError, Interchange, Segment};
use chrono::{DateTime, Utc};
use portos_core::container::{ContainerNumber, IsoSizeType};
use serde::{Deserialize, Serialize};

/// A stowage position (bay, row, tier) — tier encoded ×10 (e.g. 502 = 50.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StowPosition {
    pub bay: u32,
    pub row: u32,
    pub tier: u32,
}

impl StowPosition {
    /// Parse the LOC+147 component triple `0000102:0000401:0000502`.
    pub fn parse_triple(s: &str) -> Result<StowPosition, EdiError> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 3 {
            return Err(EdiError::InvalidValue(s.to_string(), "LOC+147 position"));
        }
        let parse_part = |p: &str| -> Result<u32, EdiError> {
            p.parse::<u32>()
                .map_err(|_| EdiError::InvalidValue(s.to_string(), "LOC+147 position"))
        };
        Ok(StowPosition {
            bay: parse_part(parts[0])?,
            row: parse_part(parts[1])?,
            tier: parse_part(parts[2])?,
        })
    }

    #[must_use]
    pub fn to_triple(&self) -> String {
        format!("{:07}:{:07}:{:07}", self.bay, self.row, self.tier)
    }
}

/// One container in the stowage plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaplieEntry {
    pub container: ContainerNumber,
    pub size_type: IsoSizeType,
    pub full: bool,
    pub weight_kg: Option<u32>,
    pub position: Option<StowPosition>,
}

/// A parsed BAPLIE message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaplieMessage {
    pub message_ref: String,
    pub vessel_name: Option<String>,
    pub voyage: Option<String>,
    pub port: Option<String>,
    pub entries: Vec<BaplieEntry>,
}

impl BaplieMessage {
    pub fn parse(ic: &Interchange) -> Result<BaplieMessage, EdiError> {
        let segs = ic
            .first_message("BAPLIE")
            .ok_or_else(|| EdiError::Unsupported("BAPLIE".into()))?;
        let mut msg = BaplieMessage {
            message_ref: String::new(),
            vessel_name: None,
            voyage: None,
            port: None,
            entries: Vec::new(),
        };
        let mut cur: Option<BaplieEntry> = None;
        for seg in segs {
            match seg.tag.as_str() {
                "UNH" => msg.message_ref = seg.el(0).unwrap_or_default().to_string(),
                "TDT" => {
                    msg.voyage = seg.el(1).map(str::to_string);
                    msg.vessel_name = seg.comp(5, 0).map(str::to_string);
                }
                "LOC" if seg.el(0) == Some("5") => msg.port = seg.el(1).map(str::to_string),
                "EQD" if seg.el(0) == Some("CN") => {
                    if let Some(e) = cur.take() {
                        msg.entries.push(e);
                    }
                    let number = seg.el(1).ok_or(EdiError::MissingElement("container number", "EQD"))?;
                    let container = ContainerNumber::parse(number)
                        .map_err(|_| EdiError::InvalidValue(number.to_string(), "EQD container"))?;
                    let size_type = seg
                        .el(2)
                        .map(IsoSizeType::parse)
                        .transpose()
                        .map_err(|_| EdiError::InvalidValue(seg.el(2).unwrap_or_default().into(), "EQD size/type"))?
                        .unwrap_or(IsoSizeType::parse("22G1").unwrap());
                    let full = seg.el(3).map(|s| s != "6").unwrap_or(true);
                    cur = Some(BaplieEntry { container, size_type, full, weight_kg: None, position: None });
                }
                "MEA" if seg.el(0) == Some("WT") => {
                    if let Some(e) = cur.as_mut() {
                        e.weight_kg = seg.comps(2).last().and_then(|v| v.parse::<u32>().ok());
                    }
                }
                "LOC" if seg.el(0) == Some("147") => {
                    // Position is a composite element: bay:row:tier.
                    let raw = seg.comps(1).join(":");
                    if let Some(e) = cur.as_mut() {
                        e.position = Some(StowPosition::parse_triple(&raw)?);
                    }
                }
                "UNT" => {
                    if let Some(e) = cur.take() {
                        msg.entries.push(e);
                    }
                }
                _ => {}
            }
        }
        if let Some(e) = cur.take() {
            msg.entries.push(e);
        }
        if msg.entries.is_empty() {
            return Err(EdiError::Unsupported("BAPLIE without EQD entries".into()));
        }
        Ok(msg)
    }

    #[must_use]
    pub fn emit(&self, sender: &str, recipient: &str, now: DateTime<Utc>) -> String {
        let mut segs = vec![envelope::unb(sender, recipient, &dt::emit_203(now), &self.message_ref)];
        segs.push(envelope::unh(&self.message_ref, "BAPLIE"));
        if let (Some(v), Some(voy)) = (&self.vessel_name, &self.voyage) {
            segs.push(Segment::build(
                "TDT",
                &[&["20"], &[voy.as_str()], &[], &[], &["SEA", "172", "20"], &[v.as_str(), "146", "20"]],
            ));
        }
        if let Some(p) = &self.port {
            segs.push(Segment::build("LOC", &[&["5"], &[p]]));
        }
        for e in &self.entries {
            let full_code = if e.full { "5" } else { "6" };
            segs.push(Segment::build(
                "EQD",
                &[&["CN"], &[e.container.as_str()], &[e.size_type.as_str()], &[full_code]],
            ));
            if let Some(w) = e.weight_kg {
                let wstr = w.to_string();
                segs.push(Segment::build("MEA", &[&["WT"], &[], &["KGM", &wstr]]));
            }
            if let Some(pos) = e.position {
                let triple = pos.to_triple();
                segs.push(Segment::build("LOC", &[&["147"], &[&triple]]));
            }
        }
        let count = segs.len() + 1;
        segs.push(envelope::unt(count, &self.message_ref));
        segs.push(envelope::unz(1, &self.message_ref));
        Interchange { una: crate::syntax::Una::default(), segments: segs }.serialize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_position_triple() {
        let p = StowPosition::parse_triple("0000102:0000401:0000502").unwrap();
        assert_eq!(p, StowPosition { bay: 102, row: 401, tier: 502 });
        assert_eq!(p.to_triple(), "0000102:0000401:0000502");
        assert!(StowPosition::parse_triple("bad").is_err());
    }

    #[test]
    fn parse_and_round_trip_baplie() {
        let raw = "UNA:+.? '\n\
            UNB+UNOC:3+LINE:ZZZ+IRBNDCT:ZZZ+261121:0700+MSG004'\n\
            UNH+MSG004+BAPLIE:D:96A:UN'\n\
            TDT+20+V212E+++SEA:172:20+GULF PEARL:146:20'\n\
            LOC+5+IRBND'\n\
            EQD+CN+MSKU3881107+22G1+++5'\n\
            MEA+WT++KGM:24000'\n\
            LOC+147+0000102:0000401:0000502'\n\
            EQD+CN+CSQU3054383+45G1+6'\n\
            LOC+147+0000502:0000401:0000502'\n\
            UNT+10+MSG004'\n\
            UNZ+1+MSG004'";
        let ic = Interchange::parse(raw).unwrap();
        let msg = BaplieMessage::parse(&ic).unwrap();
        assert_eq!(msg.vessel_name.as_deref(), Some("GULF PEARL"));
        assert_eq!(msg.entries.len(), 2);
        assert_eq!(msg.entries[0].position.unwrap(), StowPosition { bay: 102, row: 401, tier: 502 });
        assert!(!msg.entries[1].full);

        let emitted = msg.emit("LINE:ZZZ", "IRBNDCT:ZZZ", Utc::now());
        let msg2 = BaplieMessage::parse(&Interchange::parse(&emitted).unwrap()).unwrap();
        assert_eq!(msg2.entries, msg.entries);
        assert_eq!(msg2.vessel_name, msg.vessel_name);
    }
}
