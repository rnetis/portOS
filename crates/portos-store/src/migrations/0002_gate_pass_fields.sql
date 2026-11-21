-- Gate passes were persisted without the fields printed on the document,
-- so GET /v1/gate/passes/{pass_no} returned an empty pass. Store them.
ALTER TABLE gate_passes ADD COLUMN container_id TEXT;
ALTER TABLE gate_passes ADD COLUMN plate TEXT NOT NULL DEFAULT '';
ALTER TABLE gate_passes ADD COLUMN direction TEXT NOT NULL DEFAULT 'import';

-- Durable webhook delivery: the dispatcher retries with backoff and stops
-- after a terminal failure instead of dropping events on the floor.
ALTER TABLE events_outbox ADD COLUMN next_attempt_at TEXT;
ALTER TABLE events_outbox ADD COLUMN failed_at TEXT;
CREATE INDEX IF NOT EXISTS idx_outbox_due
    ON events_outbox (delivered_at, failed_at, next_attempt_at, created_at);
