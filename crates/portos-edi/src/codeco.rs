//! CODECO — Container Gate-in / Gate-out report (PRD I-2, F-18/F-19).
//!
//! Supported subset:
//! ```text
//! UNH+REF+CODECO:D:96A:UN'
//! NAD+CZ+IRBNDCT:160:20'                          (terminal)
//! EQD+CN+MSKU3881107+22G1+++5'
//! RFF+AAB+TA-2026-000042'                         (appointment reference)
//! DTM+7+202611210930:203'                         (event timestamp)
//! LOC+11+IRBNDCT:L1'                              (gate-in location: lane)
//! CUS+1'                                          (customs status code)
//! UNT+7+REF'
//! ```

use crate::dt;
use crate::syntax::{envelope, EdiError, Interchange, Segment};
use chrono::{DateTime, Utc};
use portos_core::container::{ContainerNumber, IsoSizeType};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateEvent {
    GateIn,
    GateOut,
}

/// One gate event for one container.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodecoEntry {
    pub container: ContainerNumber,
    pub size_type: IsoSizeType,
    pub event: GateEvent,
    pub lane: Option<String>,
    pub at: DateTime<Utc>,
    pub reference: Option<String>,
    /// Customs status code (1 = released, 2 = held…).
    pub customs_status: Option<String>,
}

/// A parsed CODECO message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodecoMessage {
    pub message_ref: String,
    pub terminal: Option<String>,
    pub entries: Vec<CodecoEntry>,
}

impl CodecoMessage {
    pub fn parse(ic: &Interchange) -> Result<CodecoMessage, EdiError> {
        let segs = ic
            .first_message("CODECO")
            .ok_or_else(|| EdiError::Unsupported("CODECO".into()))?;
        let mut msg = CodecoMessage {
            message_ref: String::new(),
            terminal: None,
            entries: Vec::new(),
        };
        let mut cur: Option<CodecoEntry> = None;
        for seg in segs {
            match seg.tag.as_str() {
                "UNH" => msg.message_ref = seg.el(0).unwrap_or_default().to_string(),
                "NAD" if seg.el(0) == Some("CZ") => {
                    msg.terminal = seg.comp(1, 0).map(str::to_string);
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
                                seg.el(2).unwrap_or_default().into(),
                                "EQD size/type",
                            )
                        })?
                        .unwrap_or(IsoSizeType::parse("22G1").unwrap());
                    cur = Some(CodecoEntry {
                        container,
                        size_type,
                        event: GateEvent::GateIn,
                        lane: None,
                        at: Utc::now(),
                        reference: None,
                        customs_status: None,
                    });
                }
                "RFF" if seg.el(0) == Some("AAB") => {
                    if let Some(e) = cur.as_mut() {
                        e.reference = seg.el(1).map(str::to_string);
                    }
                }
                "DTM" => {
                    if let (Some(e), Some(raw)) = (cur.as_mut(), seg.el(1)) {
                        e.at = dt::parse_203(raw.split(':').next().unwrap_or(raw))
                            .map_err(|_| EdiError::InvalidValue(raw.to_string(), "DTM"))?;
                    }
                }
                "LOC" if seg.el(0) == Some("11") => {
                    if let Some(e) = cur.as_mut() {
                        e.event = GateEvent::GateIn;
                        e.lane = Some(seg.comps(1).join(":"));
                    }
                }
                "LOC" if seg.el(0) == Some("88") => {
                    if let Some(e) = cur.as_mut() {
                        e.event = GateEvent::GateOut;
                        e.lane = Some(seg.comps(1).join(":"));
                    }
                }
                "CUS" => {
                    if let Some(e) = cur.as_mut() {
                        e.customs_status = seg.el(0).map(str::to_string);
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
            return Err(EdiError::Unsupported("CODECO without EQD entries".into()));
        }
        Ok(msg)
    }

    #[must_use]
    pub fn emit(&self, sender: &str, recipient: &str, now: DateTime<Utc>) -> String {
        let mut segs = vec![envelope::unb(
            sender,
            recipient,
            &dt::emit_203(now),
            &self.message_ref,
        )];
        segs.push(envelope::unh(&self.message_ref, "CODECO"));
        if let Some(t) = &self.terminal {
            segs.push(Segment::build("NAD", &[&["CZ"], &[t, "160", "20"]]));
        }
        for e in &self.entries {
            let loc_qual = match e.event {
                GateEvent::GateIn => "11",
                GateEvent::GateOut => "88",
            };
            let lane: String = e.lane.clone().unwrap_or_else(|| "GATE".into());
            segs.push(Segment::build(
                "EQD",
                &[&["CN"], &[e.container.as_str()], &[e.size_type.as_str()]],
            ));
            if let Some(r) = &e.reference {
                segs.push(Segment::build("RFF", &[&["AAB"], &[r]]));
            }
            let ts = dt::emit_203(e.at);
            segs.push(Segment::build("DTM", &[&["7"], &[&ts, "203"]]));
            segs.push(Segment::build("LOC", &[&[loc_qual], &[lane.as_str()]]));
            if let Some(c) = &e.customs_status {
                segs.push(Segment::build("CUS", &[&[c.as_str()]]));
            }
        }
        let count = segs.len() + 1;
        segs.push(envelope::unt(count, &self.message_ref));
        segs.push(envelope::unz(1, &self.message_ref));
        Interchange {
            una: crate::syntax::Una::default(),
            segments: segs,
        }
        .serialize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_gate_in_and_out() {
        let raw = "UNA:+.? '\n\
            UNB+UNOC:3+IRBNDCT:ZZZ+LINE:ZZZ+261121:0900+MSG002'\n\
            UNH+MSG002+CODECO:D:96A:UN'\n\
            NAD+CZ+IRBNDCT:160:20'\n\
            EQD+CN+CSQU3054383+45G1'\n\
            RFF+AAB+TA-2026-000042'\n\
            DTM+7+202611210930:203'\n\
            LOC+11+IRBNDCT:L1'\n\
            EQD+CN+MSKU3881107+22G1'\n\
            DTM+7+202611211130:203'\n\
            LOC+88+IRBNDCT:L2'\n\
            UNT+9+MSG002'\n\
            UNZ+1+MSG002'";
        let ic = Interchange::parse(raw).unwrap();
        let msg = CodecoMessage::parse(&ic).unwrap();
        assert_eq!(msg.terminal.as_deref(), Some("IRBNDCT"));
        assert_eq!(msg.entries.len(), 2);
        assert_eq!(msg.entries[0].event, GateEvent::GateIn);
        assert_eq!(msg.entries[0].lane.as_deref(), Some("IRBNDCT:L1"));
        assert_eq!(msg.entries[0].reference.as_deref(), Some("TA-2026-000042"));
        assert_eq!(msg.entries[1].event, GateEvent::GateOut);
        assert_eq!(msg.entries[1].at.to_rfc3339(), "2026-11-21T11:30:00+00:00");
    }
}
