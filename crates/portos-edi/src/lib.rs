//! PortOS EDI support (PRD I-2).
//!
//! A from-scratch EDIFACT syntax layer (UNA/UNB/UNH … UNT/UNZ segments,
//! release characters, decimal notation) plus typed mappings for the
//! container-terminal message subset:
//!
//! * COPARN — container arrival/departure list (declarations to move)
//! * CODECO — container gate-in / gate-out report
//! * COARRI — crane work report (load/discharge/shift confirmations)
//! * BAPLIE  — stowage plan (container positions by bay/row/tier)
//!
//! Each message can be parsed from EDIFACT text and serialized back,
//! enabling round-trips and interoperability tests.

pub mod baplie;
pub mod coarri;
pub mod codeco;
pub mod coparn;
pub mod dt;
pub mod syntax;

pub use syntax::{EdiError, Interchange, Segment};

/// EDIFACT syntax version this crate emits.
pub const SYNTAX_VERSION: &str = "D96A";

/// Convenience: parse raw text into an [`Interchange`].
pub fn parse(text: &str) -> Result<Interchange, EdiError> {
    syntax::Interchange::parse(text)
}
