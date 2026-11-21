//! PortOS core domain model.
//!
//! Pure, dependency-light domain types and engines shared by every other
//! crate: identifier types, value objects with industry validation
//! (ISO 6346 container numbers, IMO ship numbers, UN/LOCODE), money,
//! planning engines (berth allocation, yard slot assignment), the billing
//! engine, KPI computations, domain events and audit records.

pub mod audit;
pub mod berth;
pub mod billing;
pub mod container;
pub mod error;
pub mod events;
pub mod gate;
pub mod ids;
pub mod kpi;
pub mod money;
pub mod vessel;
pub mod work;
pub mod yard;

pub use error::{CoreError, CoreResult};

/// Format version of the domain event payloads exchanged over the API/SSE.
pub const EVENT_SCHEMA_VERSION: &str = "1.0.0";

/// Crate version, exposed at `/healthz` and `/version`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
