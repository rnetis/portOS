//! EDIFACT syntax layer: UNA service string advice, segments, elements,
//! components, release characters and interchange envelopes (UNB…UNZ).

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum EdiError {
    #[error("empty message")]
    Empty,
    #[error("missing UNA/UNB interchange header")]
    MissingHeader,
    #[error("unterminated release sequence at {0}")]
    UnterminatedRelease(usize),
    #[error("unexpected segment terminator after {0} segments")]
    TooManySegments(usize),
    #[error("invalid segment tag at byte {0}")]
    InvalidTag(usize),
    #[error("message {0} not supported by this mapping")]
    Unsupported(String),
    #[error("missing mandatory element {0} in {1}")]
    MissingElement(&'static str, &'static str),
    #[error("invalid value {0:?} for {1}")]
    InvalidValue(String, &'static str),
}

/// Default EDIFACT service string advice: `UNA:+.? '`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Una {
    /// Component data element separator (usually `:`).
    pub comp: char,
    /// Data element separator (usually `+`).
    pub elem: char,
    /// Decimal notation (usually `.`).
    pub decimal: char,
    /// Release character (usually `?`).
    pub release: char,
    /// Repetition separator (`*` in later syntax versions, reserved).
    pub repetition: char,
    /// Segment terminator (usually `'`).
    pub seg_term: char,
}

impl Default for Una {
    fn default() -> Self {
        Self {
            comp: ':',
            elem: '+',
            decimal: '.',
            release: '?',
            repetition: '*',
            seg_term: '\'',
        }
    }
}

impl Una {
    pub fn parse(s: &str) -> Result<Una, EdiError> {
        let chars: Vec<char> = s.chars().collect();
        // UNA has exactly 6 chars after "UNA".
        if chars.len() != 6 {
            return Err(EdiError::InvalidValue(s.to_string(), "UNA"));
        }
        Ok(Una {
            comp: chars[0],
            elem: chars[1],
            decimal: chars[2],
            release: chars[3],
            repetition: chars[4],
            seg_term: chars[5],
        })
    }
}

impl fmt::Display for Una {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "UNA{}{}{}{}{}{}",
            self.comp, self.elem, self.decimal, self.release, self.repetition, self.seg_term
        )
    }
}

/// One EDIFACT segment: a tag plus elements; each element is a list of
/// components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub tag: String,
    /// Elements; `None` = empty element placeholder.
    pub elements: Vec<Vec<String>>,
}

impl Segment {
    /// First component of element `idx` or `None`.
    #[must_use]
    pub fn el(&self, idx: usize) -> Option<&str> {
        self.elements
            .get(idx)
            .and_then(|e| e.first())
            .map(String::as_str)
    }

    /// Component `comp` of element `idx`.
    #[must_use]
    pub fn comp(&self, idx: usize, comp: usize) -> Option<&str> {
        self.elements
            .get(idx)
            .and_then(|e| e.get(comp))
            .map(String::as_str)
    }

    /// Element `idx` as components list.
    #[must_use]
    pub fn comps(&self, idx: usize) -> &[String] {
        static EMPTY: [String; 0] = [];
        match self.elements.get(idx) {
            Some(e) => e,
            None => &EMPTY,
        }
    }

    /// Build from tag + element/component pairs.
    #[must_use]
    pub fn build(tag: &str, elements: &[&[&str]]) -> Segment {
        Segment {
            tag: tag.to_string(),
            elements: elements
                .iter()
                .map(|comps| comps.iter().map(|s| (*s).to_string()).collect())
                .collect(),
        }
    }

    /// Serialize with the given separators, honoring release chars.
    #[must_use]
    pub fn serialize(&self, una: &Una) -> String {
        let mut out = String::new();
        out.push_str(&self.tag);
        for element in &self.elements {
            out.push(una.elem);
            let mut first = true;
            for comp in element {
                if !first {
                    out.push(una.comp);
                }
                first = false;
                for c in comp.chars() {
                    if c == una.elem || c == una.comp || c == una.release || c == una.seg_term {
                        out.push(una.release);
                    }
                    out.push(c);
                }
                if element.len() == 1 && comp.is_empty() {
                    break;
                }
            }
        }
        out.push(una.seg_term);
        out
    }
}

/// A parsed interchange: UNA + all segments in order (UNB … UNZ).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Interchange {
    pub una: Una,
    pub segments: Vec<Segment>,
}

impl Interchange {
    /// Parse raw EDIFACT text. Accepts an optional leading `UNA:+.? '` and
    /// tolerates newline-separated segments for readability.
    pub fn parse(text: &str) -> Result<Interchange, EdiError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(EdiError::Empty);
        }
        let mut una = Una::default();
        let mut rest = text;
        if let Some(stripped) = rest.strip_prefix("UNA") {
            let mut chars = stripped.chars();
            let six: String = chars.by_ref().take(6).collect();
            una = Una::parse(&six)?;
            rest = chars.as_str();
        }

        // Tokenize into raw segments: Vec<elements>, each element a Vec of
        // component strings. Elements always contain at least one component
        // (possibly empty) so separators round-trip exactly.
        let mut raw_segments: Vec<Vec<Vec<String>>> = Vec::new();
        let mut cur: Vec<Vec<String>> = vec![vec![String::new()]];
        let mut released = false;
        for c in rest.chars() {
            if released {
                if let Some(comp) = cur.last_mut().and_then(|e| e.last_mut()) {
                    comp.push(c);
                }
                released = false;
                continue;
            }
            if c == '\r' || c == '\n' {
                continue;
            }
            if c == una.release {
                released = true;
            } else if c == una.comp {
                if let Some(elem) = cur.last_mut() {
                    elem.push(String::new());
                }
            } else if c == una.elem {
                cur.push(vec![String::new()]);
            } else if c == una.seg_term {
                raw_segments.push(std::mem::replace(&mut cur, vec![vec![String::new()]]));
            } else if let Some(comp) = cur.last_mut().and_then(|e| e.last_mut()) {
                comp.push(c);
            }
        }
        // Tolerate a missing final terminator.
        if cur
            .first()
            .and_then(|e| e.first())
            .map(|t| !t.is_empty())
            .unwrap_or(false)
        {
            raw_segments.push(cur);
        }

        // The first component of the first element is the tag.
        let mut segments = Vec::with_capacity(raw_segments.len());
        for elems in raw_segments {
            let tag = elems
                .first()
                .and_then(|e| e.first())
                .cloned()
                .unwrap_or_default();
            if tag.is_empty()
                || !tag
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            {
                return Err(EdiError::InvalidTag(0));
            }
            let elements = if elems.len() > 1 {
                elems[1..].to_vec()
            } else {
                Vec::new()
            };
            segments.push(Segment { tag, elements });
        }

        if segments.is_empty() {
            return Err(EdiError::MissingHeader);
        }
        if !segments.iter().any(|s| s.tag == "UNB") {
            return Err(EdiError::MissingHeader);
        }
        Ok(Interchange { una, segments })
    }

    /// Split the first UNH..UNT message group found into its segments.
    #[must_use]
    pub fn first_message(&self, message_type: &str) -> Option<&[Segment]> {
        let mut start = None;
        let mut end = None;
        for (i, seg) in self.segments.iter().enumerate() {
            if seg.tag == "UNH" {
                let matches = seg
                    .comp(1, 0)
                    .map(|mt| mt.eq_ignore_ascii_case(message_type))
                    .unwrap_or(false);
                if matches {
                    start = Some(i);
                }
            }
            if start.is_some() && seg.tag == "UNT" {
                end = Some(i);
                break;
            }
        }
        match (start, end) {
            (Some(s), Some(e)) if e > s => Some(&self.segments[s..=e]),
            _ => None,
        }
    }

    /// Serialize back to EDIFACT text with the interchange's separators.
    #[must_use]
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.una.to_string());
        for seg in &self.segments {
            out.push_str(&seg.serialize(&self.una));
            out.push('\n');
        }
        out
    }

    /// Find a segment by tag.
    #[must_use]
    pub fn find(&self, tag: &str) -> Option<&Segment> {
        self.segments.iter().find(|s| s.tag == tag)
    }
}

/// Convenience builders for standard envelope segments.
pub mod envelope {
    use super::Segment;

    /// `UNB+UNOC:3+SENDER:ZZZ+RECIPIENT:ZZZ+YYMMDD:HHMM+REF'`
    #[must_use]
    pub fn unb(sender: &str, recipient: &str, now: &str, ref_no: &str) -> Segment {
        Segment::build(
            "UNB",
            &[
                &["UNOC", "3"],
                &[sender, "ZZZ"],
                &[recipient, "ZZZ"],
                &now.split(':').collect::<Vec<_>>(),
                &[ref_no],
            ],
        )
    }

    /// `UNZ+<count>+<ref>'`
    #[must_use]
    pub fn unz(count: usize, ref_no: &str) -> Segment {
        Segment::build("UNZ", &[&[&count.to_string()], &[ref_no]])
    }

    /// `UNH+<ref>+<type>:D:96A:UN'`
    #[must_use]
    pub fn unh(ref_no: &str, message_type: &str) -> Segment {
        Segment::build("UNH", &[&[ref_no], &[message_type, "D", "96A", "UN"]])
    }

    /// `UNT+<count>+<ref>'`
    #[must_use]
    pub fn unt(count: usize, ref_no: &str) -> Segment {
        Segment::build("UNT", &[&[&count.to_string()], &[ref_no]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_segment() {
        let ic = Interchange::parse(
            "UNA:+.? 'UNB+UNOC:3+IRBNDCT:ZZZ+LINE:ZZZ+261121:0900+REF1'\nUNZ+1+REF1'",
        )
        .unwrap();
        // UNA repeats separator is often a space (unassigned) — assert fields.
        assert_eq!(ic.una.comp, ':');
        assert_eq!(ic.una.elem, '+');
        assert_eq!(ic.una.decimal, '.');
        assert_eq!(ic.una.release, '?');
        assert_eq!(ic.una.seg_term, '\'');
        assert_eq!(ic.find("UNB").unwrap().comp(1, 0), Some("IRBNDCT"));
        assert_eq!(ic.find("UNZ").unwrap().el(1), Some("REF1"));
    }

    #[test]
    fn release_char_escapes_separators() {
        let ic = Interchange::parse(
            "UNA:+.? 'UNB+UNOC:3+S:ZZZ+R:ZZZ+261121:0900+1'\nFTX+AAI+++NAME?+WITH?+PLUS'",
        )
        .unwrap();
        let ftx = ic.segments.iter().find(|s| s.tag == "FTX").unwrap();
        // Elements: 0="AAI", 1="", 2="", 3="NAME+WITH+PLUS" (released '+').
        assert_eq!(ftx.el(3), Some("NAME+WITH+PLUS"));
        assert_eq!(ftx.el(2), Some(""));
    }

    #[test]
    fn round_trip_segments() {
        let seg = Segment::build(
            "NAD",
            &[&["CZ"], &["SHAHID RAJAEE TERMINAL", "160"], &["28", "3"]],
        );
        let s = seg.serialize(&Una::default());
        assert!(
            s.starts_with("NAD+CZ+SHAHID RAJAEE TERMINAL:160+28:3'"),
            "{s}"
        );
    }

    #[test]
    fn missing_header_errors() {
        assert!(Interchange::parse("").is_err());
        assert!(Interchange::parse("FTX+AAI+++'").is_err());
    }

    #[test]
    fn decimal_notation_default_recorded() {
        let ic =
            Interchange::parse("UNA:+.? 'UNB+UNOC:3+A:ZZZ+B:ZZZ+261121:0900+1'UNZ+1+1'").unwrap();
        assert_eq!(ic.una.decimal, '.');
    }
}
