//! COARRI — Crane Work Report (move confirmations, PRD I-2, F-13/F-32).
//!
//! Supported subset:
//! ```text
//! UNH+REF+COARRI:D:96A:UN'
//! TDT+20+V212E+++SEA:172:20+GULF PEARL:146:20'
//! EQD+CN+MSKU3881107+22G1'
//! RFF+TRN+JOB-00012'                (job reference)
//! LOC+11+QC-03'                     (crane / position)
//! DTM+203+20261122090015:204'       (move start, CCYYMMDDHHMMSS)
//! DTM+204+20261122090145:204'       (move end)
//! STS+1'                            (1 = completed, 2 = failed)
//! UNT+8+REF'
//! ```

use crate::dt;
use crate::syntax::{envelope, EdiError, Interchange, Segment};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use portos_core::container::{ContainerNumber, IsoSizeType};
use portos_core::work::JobKind;
use serde::{Deserialize, Serialize};

/// Parse `CCYYMMDDHHMMSS` (qualifier 204) as UTC.
fn parse_204(s: &str) -> Result<DateTime<Utc>, EdiError> {
    let t = s.trim();
    if t.len() != 14 || !t.chars().all(|c| c.is_ascii_digit()) {
        return Err(EdiError::InvalidValue(s.to_string(), "DTM 204"));
    }
    let naive = NaiveDateTime::parse_from_str(t, "%Y%m%d%H%M%S")
        .map_err(|_| EdiError::InvalidValue(s.to_string(), "DTM 204"))?;
    Ok(Utc.from_utc_datetime(&naive))
}

fn emit_204(dt: DateTime<Utc>) -> String {
    dt.format("%Y%m%d%H%M%S").to_string()
}

/// One confirmed crane move.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoarriEntry {
    pub container: ContainerNumber,
    pub size_type: IsoSizeType,
    pub crane: Option<String>,
    pub move_type: JobKind,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// 1 = done, 2 = failed.
    pub status: String,
    pub job_reference: Option<String>,
}

/// A parsed COARRI message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoarriMessage {
    pub message_ref: String,
    pub vessel_name: Option<String>,
    pub voyage: Option<String>,
    pub entries: Vec<CoarriEntry>,
}

impl CoarriMessage {
    pub fn parse(ic: &Interchange) -> Result<CoarriMessage, EdiError> {
        let segs = ic
            .first_message("COARRI")
            .ok_or_else(|| EdiError::Unsupported("COARRI".into()))?;
        let mut msg = CoarriMessage {
            message_ref: String::new(),
            vessel_name: None,
            voyage: None,
            entries: Vec::new(),
        };
        let mut cur: Option<CoarriEntry> = None;
        let mut cur_started: Option<DateTime<Utc>> = None;
        for seg in segs {
            match seg.tag.as_str() {
                "UNH" => msg.message_ref = seg.el(0).unwrap_or_default().to_string(),
                "TDT" => {
                    msg.voyage = seg.el(1).map(str::to_string);
                    msg.vessel_name = seg.comp(5, 0).map(str::to_string);
                }
                "EQD" if seg.el(0) == Some("CN") => {
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
                    cur = Some(CoarriEntry {
                        container,
                        size_type,
                        crane: None,
                        move_type: JobKind::Discharge,
                        started_at: Utc::now(),
                        finished_at: None,
                        status: "1".into(),
                        job_reference: None,
                    });
                    cur_started = None;
                }
                "RFF" if seg.el(0) == Some("TRN") => {
                    if let Some(e) = cur.as_mut() {
                        e.job_reference = seg.el(1).map(str::to_string);
                    }
                }
                "LOC" if seg.el(0) == Some("11") => {
                    if let Some(e) = cur.as_mut() {
                        e.crane = seg.el(1).map(str::to_string);
                    }
                }
                "DTM" if seg.el(0) == Some("203") => {
                    if let (Some(e), Some(raw)) = (cur.as_mut(), seg.el(1)) {
                        let started = parse_204(raw.split(':').next().unwrap_or(raw))?;
                        e.started_at = started;
                        cur_started = Some(started);
                    }
                }
                "DTM" if seg.el(0) == Some("204") => {
                    if let (Some(e), Some(raw)) = (cur.as_mut(), seg.el(1)) {
                        e.finished_at = Some(parse_204(raw.split(':').next().unwrap_or(raw))?);
                    }
                }
                "STS" => {
                    if let (Some(e), Some(code)) = (cur.as_mut(), seg.el(0)) {
                        e.status = code.to_string();
                    }
                }
                "UNT" => {
                    if let Some(e) = cur.take() {
                        if cur_started.is_some() && e.started_at == Utc::now() {
                            // keep as parsed
                        }
                        msg.entries.push(e);
                        cur_started = None;
                    }
                }
                _ => {}
            }
        }
        if let Some(e) = cur.take() {
            msg.entries.push(e);
        }
        if msg.entries.is_empty() {
            return Err(EdiError::Unsupported("COARRI without EQD entries".into()));
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
        segs.push(envelope::unh(&self.message_ref, "COARRI"));
        for e in &self.entries {
            segs.push(Segment::build(
                "EQD",
                &[&["CN"], &[e.container.as_str()], &[e.size_type.as_str()]],
            ));
            if let Some(r) = &e.job_reference {
                segs.push(Segment::build("RFF", &[&["TRN"], &[r]]));
            }
            if let Some(c) = &e.crane {
                segs.push(Segment::build("LOC", &[&["11"], &[c]]));
            }
            let s = emit_204(e.started_at);
            segs.push(Segment::build("DTM", &[&["203"], &[&s, "204"]]));
            if let Some(f) = e.finished_at {
                let f = emit_204(f);
                segs.push(Segment::build("DTM", &[&["204"], &[&f, "204"]]));
            }
            segs.push(Segment::build("STS", &[&[e.status.as_str()]]));
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
    use portos_core::work::JobKind;

    #[test]
    fn parse_crane_report() {
        let raw = "UNA:+.? '\n\
            UNB+UNOC:3+IRBNDCT:ZZZ+LINE:ZZZ+261122:0901+MSG003'\n\
            UNH+MSG003+COARRI:D:96A:UN'\n\
            TDT+20+V212E+++SEA:172:20+GULF PEARL:146:20'\n\
            EQD+CN+MSKU3881107+22G1'\n\
            RFF+TRN+JOB-00012'\n\
            LOC+11+QC-03'\n\
            DTM+203+20261122090015:204'\n\
            DTM+204+20261122090145:204'\n\
            STS+1'\n\
            UNT+9+MSG003'\n\
            UNZ+1+MSG003'";
        let ic = Interchange::parse(raw).unwrap();
        let msg = CoarriMessage::parse(&ic).unwrap();
        assert_eq!(msg.entries.len(), 1);
        let e = &msg.entries[0];
        assert_eq!(e.crane.as_deref(), Some("QC-03"));
        assert_eq!(e.job_reference.as_deref(), Some("JOB-00012"));
        assert_eq!(e.started_at.to_rfc3339(), "2026-11-22T09:00:15+00:00");
        assert_eq!(
            e.finished_at.unwrap().to_rfc3339(),
            "2026-11-22T09:01:45+00:00"
        );
        assert_eq!(e.status, "1");
        assert_eq!(e.move_type, JobKind::Discharge);

        let emitted = msg.emit("IRBNDCT:ZZZ", "LINE:ZZZ", Utc::now());
        let msg2 = CoarriMessage::parse(&Interchange::parse(&emitted).unwrap()).unwrap();
        assert_eq!(msg2.entries, msg.entries);
    }
}
