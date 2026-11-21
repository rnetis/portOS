//! Typed identifiers. Newtypes over UUIDv7 (time-ordered, index-friendly).

/// Generate a strongly-typed identifier newtype.
macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord,
            serde::Serialize, serde::Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(uuid::Uuid);

        impl $name {
            /// New unique id (UUIDv7, sortable by creation time).
            #[must_use]
            pub fn generate() -> Self {
                Self(uuid::Uuid::now_v7())
            }

            #[must_use]
            pub const fn from_uuid(u: uuid::Uuid) -> Self {
                Self(u)
            }

            #[must_use]
            pub const fn as_uuid(&self) -> uuid::Uuid {
                self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                uuid::Uuid::parse_str(s).map(Self)
            }
        }
    };
}

define_id!(
    /// A registered organisation that interacts with the terminal
    /// (shipping line, agent, forwarder, trucking company, customs…).
    CustomerId
);
define_id!(/// A vessel registered in master data (PRD F-47).
    VesselId
);
define_id!(/// A vessel call / port visit (PRD F-1).
    CallId
);
define_id!(/// A berth along the quay.
    BerthId
);
define_id!(/// A berth allocation decision (PRD F-2).
    AllocationId
);
define_id!(/// A yard block (stack area).
    BlockId
);
define_id!(/// A single yard slot position (bay/row/tier).
    SlotId
);
define_id!(/// A truck appointment time slot (PRD F-17).
    AppointmentId
);
define_id!(/// A gate transaction (PRD F-18).
    GateTxId
);
define_id!(/// A tariff definition (PRD F-27).
    TariffId
);
define_id!(/// An invoice (PRD F-29).
    InvoiceId
);
define_id!(/// A user account.
    UserId
);
define_id!(/// A work job (discharge/load/shift) dispatched to equipment (PRD F-13).
    JobId
);
define_id!(/// A recorded physical move.
    MoveId
);
define_id!(/// A piece of terminal equipment.
    EquipmentId
);
define_id!(/// A customs/security/line hold (PRD F-11).
    HoldId
);
define_id!(/// A reefer telemetry reading (PRD F-10).
    ReadingId
);
define_id!(/// A safety incident report (PRD F-38).
    IncidentId
);
define_id!(/// An outbound webhook subscription (PRD I-1).
    WebhookId
);
