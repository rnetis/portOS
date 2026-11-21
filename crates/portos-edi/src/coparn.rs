//! COPARN — Container Arriving / Departing list (announced movements,
//! PRD I-2 / F-6 input side).
//!
//! Supported subset:
//! ```text
//! UNH+REF+COPARN:D:96A:UN'
//! TDT+20+VOYAGE+++SEA:172:20+VESSEL NAME:146:20'
//! LOC+5+IRBND'
//! DTM+132+202611210930:203'          (ETA)
//! EQD+CN+MSKU3881107+22G1+++5'       (equipment, status 5=full/6=empty*)
//! MEA+WT++KGM:24000'
//! STS+1'                             (movement: 1=load, 2=discharge, 3=shift)
//! UNT+8+REF'
//! ```
//! *Empty/full indicator follows the ISO status codes used by carriers.

use crate::dt;
use crate::syntax::{envelope, EdiError, Interchange, Segment};
use chrono::{DateTime, Utc};
use portos_core::container::{ContainerNumber, IsoSizeType};
use serde::{Deserialize, Serialize};

/// Movement type announced for the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Movement {
    Load,
    Discharge,
    Shift,
}

impl Movement {
    fn from_status_code(code: &str) -> Result<Movement, EdiError> {
        match code {
            "1" => Ok(Movement::Load),
            "2" => Ok(Movement::Discharge),
            "3" => Ok(Movement::Shift),
            other => Err(EdiError::InvalidValue(other.to_string(), "STS movement")),
        }
    }

    fn to_status_code(self) -> &'static str {
        match self {
            Movement::Load => "1",
            Movement::Discharge => "2",
            Movement::Shift => "3",
        }
    }
}

/// One announced container movement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoparnEntry {
    pub container: ContainerNumber,
    pub size_type: IsoSizeType,
    pub movement: Movement,
    pub full: bool,
    pub weight_kg: Option<u32>,
}

/// A parsed COPARN message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoparnMessage {
    pub message_ref: String,
    pub vessel_name: Option<String>,
    pub voyage: Option<String>,
    pub port: Option<String>,
    pub eta_utc: Option<DateTime<Utc>>,
    pub entries: Vec<CoparnEntry>,
}

impl CoparnMessage {
    /// Parse from a full interchange (UNB…UNZ optional).
    pub fn parse(ic: &Interchange) -> Result<CoparnMessage, EdiError> {
        let segs = ic
            .first_message("COPARN")
            .ok_or_else(|| EdiError::Unsupported("COPARN".into()))?;
        let mut msg = CoparnMessage {
            message_ref: String::new(),
            vessel_name: None,
            voyage: None,
            port: None,
            eta_utc: None,
            entries: Vec::new(),
        };
        let mut cur: Option<CoparnEntry> = None;
        for seg in segs {
            match seg.tag.as_str() {
                "UNH" => {
                    msg.message_ref = seg.el(0).unwrap_or_default().to_string();
                }
                "TDT" => {
                    msg.voyage = seg.el(1).map(str::to_string);
                    msg.vessel_name = seg.comp(5, 0).map(str::to_string);
                }
                "LOC" if seg.el(0) == Some("5") => {
                    msg.port = seg.el(1).map(str::to_string);
                }
                "DTM" if seg.el(0) == Some("132") => {
                    if let Some(raw) = seg.el(1) {
                        msg.eta_utc = Some(
                            dt::parse_203(raw)
                                .map_err(|_| EdiError::InvalidValue(raw.to_string(), "DTM+132"))?,
                        );
                    }
                }
                "EQD" => {
                    if seg.el(0) != Some("CN") {
                        continue;
                    }
                    if let Some(e) = cur.take() {
                        msg.entries.push(e);
                    }
                    let number = seg
                        .el(1)
                        .ok_or(EdiError::MissingElement("container number", "EQD"))?;
                    let container = ContainerNumber::parse(number)
                        .map_err(|_| EdiError::InvalidValue(number.to_string(), "EQD container"))?;
                    let size_type = seg
                        .el(2)
                        .map(IsoSizeType::parse)
                        .transpose()
                        .map_err(|_| {
                            EdiError::InvalidValue(
                                seg.el(2).unwrap_or_default().to_string(),
                                "EQD size/type",
                            )
                        })?
                        .unwrap_or(IsoSizeType::parse("22G1").unwrap());
                    // EQD+CN+NUM+SIZE+<status 5=full/6=empty>; movement in STS.
                    let full = seg.el(3).map(|s| s != "6").unwrap_or(true);
                    let movement = if full {
                        Movement::Load
                    } else {
                        Movement::Discharge
                    };
                    cur = Some(CoparnEntry {
                        container,
                        size_type,
                        movement,
                        full,
                        weight_kg: None,
                    });
                }
                "STS" => {
                    if let Some(e) = cur.as_mut() {
                        e.movement = match seg.el(0) {
                            Some(code) => Movement::from_status_code(code)?,
                            None => continue,
                        };
                    }
                }
                "MEA" if seg.el(0) == Some("WT") => {
                    if let Some(entry) = cur.as_mut() {
                        // Element 2 is a composite: KGM:24000 — take last comp.
                        let val = seg.comps(2).last().and_then(|v| v.parse::<u32>().ok());
                        entry.weight_kg = val;
                    }
                }
                "UNT" => {
                    if let Some(entry) = cur.take() {
                        msg.entries.push(entry);
                    }
                }
                _ => {}
            }
        }
        if let Some(entry) = cur.take() {
            msg.entries.push(entry);
        }
        if msg.entries.is_empty() {
            return Err(EdiError::Unsupported("COPARN without EQD entries".into()));
        }
        Ok(msg)
    }

    /// Serialize to a full interchange text with envelope.
    #[must_use]
    pub fn emit(&self, sender: &str, recipient: &str, now: DateTime<Utc>) -> String {
        let mut segs = vec![envelope::unb(
            sender,
            recipient,
            &dt::emit_203(now),
            &self.message_ref,
        )];
        segs.push(envelope::unh(&self.message_ref, "COPARN"));
        let mut tdt_elements: Vec<&[&str]> = vec![&["20"]];
        let voyage: Vec<&str> = self
            .voyage
            .as_ref()
            .map(|v| vec![v.as_str()])
            .unwrap_or_default();
        if !voyage.is_empty() {
            tdt_elements.push(&voyage);
        }
        // Two empty elements keep vessel name at composite index 5
        // (TDT+20+<voyage>+++SEA:172:20+<name>:146:20).
        tdt_elements.push(&[]);
        tdt_elements.push(&[]);
        let sea_ref = ["SEA", "172", "20"];
        tdt_elements.push(&sea_ref);
        let vessel_ref: Vec<&str> = self
            .vessel_name
            .as_ref()
            .map(|v| vec![v.as_str(), "146", "20"])
            .unwrap_or_default();
        if !vessel_ref.is_empty() {
            tdt_elements.push(&vessel_ref);
        }
        segs.push(Segment {
            tag: "TDT".into(),
            elements: tdt_elements
                .iter()
                .map(|e| e.iter().map(|s| (*s).to_string()).collect())
                .collect(),
        });
        if let Some(port) = &self.port {
            segs.push(Segment::build("LOC", &[&["5"], &[port]]));
        }
        if let Some(eta) = self.eta_utc {
            let raw = dt::emit_203(eta);
            segs.push(Segment::build("DTM", &[&["132"], &[&raw, "203"]]));
        }
        for e in &self.entries {
            let status = e.movement.to_status_code();
            let full_code = if e.full { "5" } else { "6" };
            segs.push(Segment::build(
                "EQD",
                &[
                    &["CN"],
                    &[e.container.as_str()],
                    &[e.size_type.as_str()],
                    &[full_code],
                ],
            ));
            segs.push(Segment::build("STS", &[&[status]]));
            if let Some(w) = e.weight_kg {
                let wstr = w.to_string();
                segs.push(Segment::build("MEA", &[&["WT"], &[], &["KGM", &wstr]]));
            }
        }
        let count = segs.len() + 1;
        segs.push(envelope::unt(count, &self.message_ref));
        segs.push(envelope::unz(1, &self.message_ref));
        let ic = Interchange {
            una: crate::syntax::Una::default(),
            segments: segs,
        };
        ic.serialize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::Una;

    #[test]
    fn parse_and_emit_round_trip() {
        let raw = "UNA:+.? '\n\
            UNB+UNOC:3+LINE:ZZZ+IRBNDCT:ZZZ+261121:0800+MSG001'\n\
            UNH+MSG001+COPARN:D:96A:UN'\n\
            TDT+20+V212E+++SEA:172:20+GULF PEARL:146:20'\n\
            LOC+5+IRBND'\n\
            DTM+132+202611220900:203'\n\
            EQD+CN+MSKU3881107+22G1+5'\n\
            STS+1'\n\
            MEA+WT++KGM:24000'\n\
            UNT+9+MSG001'\n\
            UNZ+1+MSG001'";
        let ic = Interchange::parse(raw).unwrap();
        let msg = CoparnMessage::parse(&ic).unwrap();
        assert_eq!(msg.vessel_name.as_deref(), Some("GULF PEARL"));
        assert_eq!(msg.voyage.as_deref(), Some("V212E"));
        assert_eq!(msg.port.as_deref(), Some("IRBND"));
        assert_eq!(msg.entries.len(), 1);
        assert_eq!(msg.entries[0].container.as_str(), "MSKU3881107");
        assert_eq!(msg.entries[0].movement, Movement::Load);
        assert!(msg.entries[0].full);
        assert_eq!(msg.entries[0].weight_kg, Some(24_000));
        assert_eq!(
            msg.eta_utc.unwrap().to_rfc3339(),
            "2026-11-22T09:00:00+00:00"
        );

        let emitted = msg.emit("LINE:ZZZ", "IRBNDCT:ZZZ", Utc::now());
        let ic2 = Interchange::parse(&emitted).unwrap();
        let msg2 = CoparnMessage::parse(&ic2).unwrap();
        assert_eq!(msg2.entries, msg.entries);
        assert_eq!(msg2.vessel_name, msg.vessel_name);
        let _ = Una::default();
    }
}
