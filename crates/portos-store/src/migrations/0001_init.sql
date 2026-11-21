-- PortOS Phase 1 schema. Timestamps are ISO 8601 UTC text; money is stored
-- in integer minor units; JSON payloads are TEXT.

CREATE TABLE users (
    id            TEXT PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    role          TEXT NOT NULL,
    locale        TEXT NOT NULL DEFAULT 'en',
    calendar      TEXT NOT NULL DEFAULT 'gregorian',
    numerals      TEXT NOT NULL DEFAULT 'western',
    active        INTEGER NOT NULL DEFAULT 1,
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);

CREATE TABLE customers (
    id                 TEXT PRIMARY KEY,
    name_en            TEXT NOT NULL,
    name_fa            TEXT NOT NULL,
    kind               TEXT NOT NULL,            -- line | agent | forwarder | trucker | customs | terminal
    credit_limit_minor INTEGER NOT NULL DEFAULT 0,
    currency           TEXT NOT NULL DEFAULT 'IRR',
    active             INTEGER NOT NULL DEFAULT 1,
    created_at         TEXT NOT NULL
);

CREATE TABLE vessels (
    id           TEXT PRIMARY KEY,
    imo          TEXT NOT NULL UNIQUE,
    name_en      TEXT NOT NULL,
    name_fa      TEXT NOT NULL,
    mmsi         TEXT,
    flag_code    TEXT,
    loa_m        REAL NOT NULL,
    beam_m       REAL NOT NULL,
    max_draft_m  REAL NOT NULL,
    dwt_t        INTEGER NOT NULL,
    reefer_points INTEGER NOT NULL DEFAULT 0,
    gearless     INTEGER NOT NULL DEFAULT 0,
    customer_id  TEXT REFERENCES customers(id)
);

CREATE TABLE berths (
    id            TEXT PRIMARY KEY,
    code          TEXT NOT NULL UNIQUE,
    name_en       TEXT NOT NULL,
    name_fa       TEXT NOT NULL,
    length_m      REAL NOT NULL,
    max_draught_m REAL NOT NULL,
    crane_count   INTEGER NOT NULL,
    shore_power   INTEGER NOT NULL DEFAULT 0,
    capabilities  TEXT NOT NULL,                 -- JSON array
    active        INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE vessel_calls (
    id                  TEXT PRIMARY KEY,
    vessel_id           TEXT NOT NULL REFERENCES vessels(id),
    voyage_code         TEXT NOT NULL,
    port                TEXT NOT NULL,
    eta_utc             TEXT NOT NULL,
    etd_utc             TEXT,
    status              TEXT NOT NULL,
    cargo_mix           TEXT NOT NULL,           -- JSON
    anchorage_arrived_at TEXT,
    berthed_at          TEXT,
    work_completed_at   TEXT,
    departed_at         TEXT,
    created_by          TEXT NOT NULL,
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    UNIQUE (vessel_id, voyage_code)
);
CREATE INDEX idx_calls_eta ON vessel_calls (eta_utc);
CREATE INDEX idx_calls_status ON vessel_calls (status);

CREATE TABLE berth_requests (
    id             TEXT PRIMARY KEY,
    call_id        TEXT NOT NULL REFERENCES vessel_calls(id),
    requested_from TEXT NOT NULL,
    requested_to   TEXT NOT NULL,
    priority       INTEGER NOT NULL DEFAULT 3,
    reason         TEXT,
    status         TEXT NOT NULL DEFAULT 'open', -- open | allocated | cancelled
    created_at     TEXT NOT NULL
);

CREATE TABLE berth_allocations (
    id           TEXT PRIMARY KEY,
    call_id      TEXT NOT NULL REFERENCES vessel_calls(id),
    berth_id     TEXT NOT NULL REFERENCES berths(id),
    planned_from TEXT NOT NULL,
    planned_to   TEXT NOT NULL,
    score        REAL,
    reasons      TEXT,                           -- JSON array
    decided_by   TEXT NOT NULL,
    decided_at   TEXT NOT NULL
);
CREATE INDEX idx_alloc_berth ON berth_allocations (berth_id, planned_from, planned_to);

CREATE TABLE containers (
    id                 TEXT PRIMARY KEY,          -- ISO 6346 number
    size_type          TEXT NOT NULL,
    weight_kg          INTEGER NOT NULL DEFAULT 0,
    imdg_class         TEXT,
    reefer_setpoint_c  REAL,
    over_gauge         INTEGER NOT NULL DEFAULT 0,
    seal_number        TEXT,
    customer_id        TEXT REFERENCES customers(id),
    pol                TEXT,
    pod                TEXT,
    state              TEXT NOT NULL DEFAULT 'advertised',
    customs_status     TEXT NOT NULL DEFAULT 'not_declared',
    gate_in_at         TEXT,
    gate_out_at        TEXT,
    created_at         TEXT NOT NULL,
    updated_at         TEXT NOT NULL
);
CREATE INDEX idx_containers_state ON containers (state);

CREATE TABLE yard_blocks (
    id                 TEXT PRIMARY KEY,
    code               TEXT NOT NULL UNIQUE,
    name_en            TEXT NOT NULL,
    name_fa            TEXT NOT NULL,
    block_type         TEXT NOT NULL,
    bays               INTEGER NOT NULL,
    rows               INTEGER NOT NULL,
    tiers              INTEGER NOT NULL,
    reefer_plugs       INTEGER NOT NULL DEFAULT 0,
    distance_to_quay_m INTEGER NOT NULL DEFAULT 500,
    distance_to_gate_m INTEGER NOT NULL DEFAULT 500
);

CREATE TABLE yard_slots (
    id           TEXT PRIMARY KEY,
    block_id     TEXT NOT NULL REFERENCES yard_blocks(id),
    bay          INTEGER NOT NULL,
    row          INTEGER NOT NULL,
    tier         INTEGER NOT NULL,
    state        TEXT NOT NULL DEFAULT 'free',
    container_id TEXT REFERENCES containers(id),
    UNIQUE (block_id, bay, row, tier)
);
CREATE INDEX idx_slots_block ON yard_slots (block_id, state);

CREATE TABLE yard_moves (
    id           TEXT PRIMARY KEY,
    container_id TEXT NOT NULL,
    from_slot    TEXT,
    to_slot      TEXT,
    reason       TEXT,
    at           TEXT NOT NULL,
    by           TEXT
);

CREATE TABLE holds (
    id           TEXT PRIMARY KEY,
    container_id TEXT NOT NULL,
    kind         TEXT NOT NULL,
    reason       TEXT NOT NULL,
    placed_by    TEXT NOT NULL,
    placed_at    TEXT NOT NULL,
    released_at  TEXT,
    released_by  TEXT
);
CREATE INDEX idx_holds_container ON holds (container_id, released_at);

CREATE TABLE reefer_readings (
    id            TEXT PRIMARY KEY,
    container_id  TEXT NOT NULL,
    at            TEXT NOT NULL,
    temperature_c REAL NOT NULL,
    power_ok      INTEGER NOT NULL,
    alarm         TEXT
);
CREATE INDEX idx_reefer_container ON reefer_readings (container_id, at);

CREATE TABLE truck_appointments (
    id                  TEXT PRIMARY KEY,
    reference           TEXT NOT NULL UNIQUE,
    trucking_company_id TEXT REFERENCES customers(id),
    driver_name         TEXT NOT NULL,
    driver_license      TEXT NOT NULL,
    plate               TEXT NOT NULL,
    container_id        TEXT,
    direction           TEXT NOT NULL,
    slot_from           TEXT NOT NULL,
    slot_to             TEXT NOT NULL,
    status              TEXT NOT NULL DEFAULT 'booked',
    created_at          TEXT NOT NULL
);
CREATE INDEX idx_appt_slot ON truck_appointments (slot_from, status);

CREATE TABLE gate_transactions (
    id                 TEXT PRIMARY KEY,
    appointment_id     TEXT REFERENCES truck_appointments(id),
    direction          TEXT NOT NULL,
    expected_container TEXT,
    ocr_container      TEXT,
    plate              TEXT NOT NULL,
    driver_license     TEXT NOT NULL,
    seal_number        TEXT,
    gate_lane          TEXT NOT NULL,
    steps              TEXT NOT NULL DEFAULT '[]', -- JSON array
    status             TEXT NOT NULL DEFAULT 'in_progress',
    reject_reason      TEXT,
    started_at         TEXT NOT NULL,
    completed_at       TEXT,
    idempotency_key    TEXT UNIQUE
);
CREATE INDEX idx_gate_started ON gate_transactions (started_at);

CREATE TABLE gate_passes (
    pass_no     TEXT PRIMARY KEY,
    gate_tx_id  TEXT NOT NULL REFERENCES gate_transactions(id),
    issued_at   TEXT NOT NULL,
    valid_until TEXT NOT NULL,
    qr_payload  TEXT NOT NULL
);

CREATE TABLE eirs (
    eir_no       TEXT PRIMARY KEY,
    gate_tx_id   TEXT NOT NULL REFERENCES gate_transactions(id),
    container_id TEXT NOT NULL,
    condition    TEXT NOT NULL,
    damages      TEXT NOT NULL DEFAULT '[]',
    signed_by    TEXT NOT NULL,
    at           TEXT NOT NULL
);

CREATE TABLE equipment (
    id              TEXT PRIMARY KEY,
    code            TEXT NOT NULL UNIQUE,
    kind            TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'available',
    hours_meter     REAL NOT NULL DEFAULT 0,
    last_service_at TEXT,
    telemetry       TEXT
);

CREATE TABLE jobs (
    id           TEXT PRIMARY KEY,
    kind         TEXT NOT NULL,
    call_id      TEXT REFERENCES vessel_calls(id),
    container_id TEXT,
    assigned_to  TEXT REFERENCES equipment(id),
    priority     INTEGER NOT NULL DEFAULT 5,
    status       TEXT NOT NULL DEFAULT 'queued',
    created_at   TEXT NOT NULL,
    assigned_at  TEXT,
    completed_at TEXT,
    issue        TEXT
);
CREATE INDEX idx_jobs_status ON jobs (status, priority, created_at);

CREATE TABLE moves (
    id           TEXT PRIMARY KEY,
    job_id       TEXT NOT NULL REFERENCES jobs(id),
    equipment_id TEXT NOT NULL REFERENCES equipment(id),
    container_id TEXT NOT NULL,
    kind         TEXT NOT NULL,
    started_at   TEXT NOT NULL,
    finished_at  TEXT NOT NULL,
    success      INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX idx_moves_time ON moves (started_at, finished_at);

CREATE TABLE tariffs (
    id          TEXT PRIMARY KEY,
    code        TEXT NOT NULL UNIQUE,
    name_en     TEXT NOT NULL,
    name_fa     TEXT NOT NULL,
    basis       TEXT NOT NULL,
    currency    TEXT NOT NULL,
    unit_minor  INTEGER NOT NULL,
    free_days   INTEGER NOT NULL DEFAULT 0,
    tiers       TEXT NOT NULL DEFAULT '[]',
    tax_rate    REAL NOT NULL DEFAULT 0,
    customer_id TEXT REFERENCES customers(id),
    active      INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE fx_rates (
    base   TEXT NOT NULL,
    quote  TEXT NOT NULL,
    rate   REAL NOT NULL,
    as_of  TEXT NOT NULL,
    source TEXT NOT NULL DEFAULT 'manual',
    PRIMARY KEY (base, quote)
);

CREATE TABLE invoices (
    id             TEXT PRIMARY KEY,
    number         TEXT NOT NULL UNIQUE,
    customer_id    TEXT NOT NULL REFERENCES customers(id),
    currency       TEXT NOT NULL,
    status         TEXT NOT NULL DEFAULT 'draft',
    subtotal_minor INTEGER NOT NULL,
    tax_minor      INTEGER NOT NULL,
    total_minor    INTEGER NOT NULL,
    paid_minor     INTEGER NOT NULL DEFAULT 0,
    issued_at      TEXT NOT NULL,
    due_at         TEXT NOT NULL,
    notes          TEXT
);
CREATE INDEX idx_invoices_customer ON invoices (customer_id, issued_at);

CREATE TABLE invoice_lines (
    id              TEXT PRIMARY KEY,
    invoice_id      TEXT NOT NULL REFERENCES invoices(id),
    tariff_code     TEXT NOT NULL,
    description_en  TEXT NOT NULL,
    description_fa  TEXT NOT NULL,
    ref_type        TEXT NOT NULL,
    ref_id          TEXT NOT NULL,
    quantity        REAL NOT NULL,
    unit_minor      INTEGER NOT NULL,
    amount_minor    INTEGER NOT NULL,
    tax_rate        REAL NOT NULL,
    tax_minor       INTEGER NOT NULL
);
CREATE INDEX idx_lines_invoice ON invoice_lines (invoice_id);

CREATE TABLE payments (
    id           TEXT PRIMARY KEY,
    invoice_id   TEXT NOT NULL REFERENCES invoices(id),
    amount_minor INTEGER NOT NULL,
    currency     TEXT NOT NULL,
    paid_at      TEXT NOT NULL,
    method       TEXT NOT NULL,
    reference    TEXT
);

CREATE TABLE incidents (
    id          TEXT PRIMARY KEY,
    kind        TEXT NOT NULL,
    severity    TEXT NOT NULL,
    location    TEXT,
    description TEXT NOT NULL,
    occurred_at TEXT NOT NULL,
    reported_by TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'open',
    actions     TEXT NOT NULL DEFAULT '[]'
);

CREATE TABLE audit_events (
    id          TEXT PRIMARY KEY,
    at          TEXT NOT NULL,
    actor       TEXT NOT NULL,
    action      TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    details     TEXT
);
CREATE INDEX idx_audit_time ON audit_events (at);
CREATE INDEX idx_audit_entity ON audit_events (entity_type, entity_id);

CREATE TABLE events_outbox (
    id           TEXT PRIMARY KEY,
    event_type   TEXT NOT NULL,
    payload      TEXT NOT NULL,
    created_at   TEXT NOT NULL,
    delivered_at TEXT,
    attempts     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_outbox_pending ON events_outbox (delivered_at, created_at);

CREATE TABLE webhooks (
    id         TEXT PRIMARY KEY,
    url        TEXT NOT NULL,
    secret     TEXT NOT NULL,
    events     TEXT NOT NULL DEFAULT '*',   -- comma-separated event types or '*'
    active     INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL
);
