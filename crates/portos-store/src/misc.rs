//! Cross-cutting repositories: audit trail, event outbox, webhooks and
//! safety incidents (PRD F-38, F-40, I-1).

use crate::util::{map_err, on_conn, parse_ts, ts};
use crate::Store;
use chrono::{DateTime, Utc};
use portos_core::audit::AuditRecord;
use portos_core::ids::{IncidentId, WebhookId};
use portos_core::{CoreError, CoreResult};
use rusqlite::Row;

fn bad_row(col: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(0, col.to_string(), rusqlite::types::Type::Text)
}

// --------------------------------------------------------------------------
// Audit (F-40)
// --------------------------------------------------------------------------

pub fn append_audit(store: &Store, record: &AuditRecord) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO audit_events (id, at, actor, action, entity_type, entity_id, details) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            rusqlite::params![
                record.id.to_string(), ts(record.at), record.actor, record.action,
                record.entity_type, record.entity_id,
                record.details.as_ref().map(|d| d.to_string())
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn query_audit(
    store: &Store,
    actor: Option<&str>,
    entity_type: Option<&str>,
    from: Option<DateTime<Utc>>,
    limit: u32,
) -> CoreResult<Vec<AuditRecord>> {
    on_conn(&store.conn(), |c| {
        let mut sql = String::from("SELECT * FROM audit_events WHERE 1=1");
        let mut binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(actor) = actor {
            sql.push_str(&format!(" AND actor = ?{}", binds.len() + 1));
            binds.push(Box::new(actor.to_string()));
        }
        if let Some(entity_type) = entity_type {
            sql.push_str(&format!(" AND entity_type = ?{}", binds.len() + 1));
            binds.push(Box::new(entity_type.to_string()));
        }
        if let Some(from) = from {
            sql.push_str(&format!(" AND at >= ?{}", binds.len() + 1));
            binds.push(Box::new(ts(from)));
        }
        sql.push_str(&format!(" ORDER BY at DESC LIMIT ?{}", binds.len() + 1));
        binds.push(Box::new(limit));

        let mut stmt = c.prepare(&sql).map_err(map_err)?;
        let pref: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|b| b.as_ref()).collect();
        let rows = stmt
            .query_map(pref.as_slice(), |r| {
                Ok(AuditRecord {
                    id: uuid::Uuid::parse_str(&r.get::<_, String>("id")?)
                        .map_err(|_| bad_row("id"))?,
                    at: parse_ts(&r.get::<_, String>("at")?).map_err(|_| bad_row("at"))?,
                    actor: r.get("actor")?,
                    action: r.get("action")?,
                    entity_type: r.get("entity_type")?,
                    entity_id: r.get("entity_id")?,
                    details: r
                        .get::<_, Option<String>>("details")?
                        .and_then(|d| serde_json::from_str(&d).ok()),
                })
            })
            .map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
    })
}

// --------------------------------------------------------------------------
// Event outbox (transactional outbox pattern; I-1, F-26)
// --------------------------------------------------------------------------

pub fn append_event(
    store: &Store,
    id: uuid::Uuid,
    event_type: &str,
    payload_json: &str,
    at: DateTime<Utc>,
) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO events_outbox (id, event_type, payload, created_at) VALUES (?1,?2,?3,?4)",
            rusqlite::params![id.to_string(), event_type, payload_json, ts(at)],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn pending_events(
    store: &Store,
    limit: u32,
) -> CoreResult<Vec<(uuid::Uuid, String, String, i64)>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT id, event_type, payload, attempts FROM events_outbox WHERE delivered_at IS NULL AND failed_at IS NULL ORDER BY created_at LIMIT ?1")
            .map_err(map_err)?;
        let rows = stmt
            .query_map([limit], |r| {
                Ok((
                    uuid::Uuid::parse_str(&r.get::<_, String>(0)?).map_err(|_| bad_row("id"))?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
    })
}

pub fn mark_event_delivered(store: &Store, id: uuid::Uuid) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "UPDATE events_outbox SET delivered_at = ?2, next_attempt_at = NULL WHERE id = ?1",
            rusqlite::params![id.to_string(), ts(Utc::now())],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn bump_event_attempts(store: &Store, id: uuid::Uuid) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "UPDATE events_outbox SET attempts = attempts + 1 WHERE id = ?1",
            [id.to_string()],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

/// Outbox rows that are due for (re)delivery: never delivered, not
/// terminally failed, and past their scheduled retry time.
pub fn due_events(
    store: &Store,
    now: DateTime<Utc>,
    limit: u32,
) -> CoreResult<Vec<(uuid::Uuid, String, String, i64)>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare(
                "SELECT id, event_type, payload, attempts FROM events_outbox
                 WHERE delivered_at IS NULL AND failed_at IS NULL
                   AND (next_attempt_at IS NULL OR next_attempt_at <= ?1)
                 ORDER BY created_at LIMIT ?2",
            )
            .map_err(map_err)?;
        let rows = stmt
            .query_map(rusqlite::params![ts(now), limit], |r| {
                Ok((
                    uuid::Uuid::parse_str(&r.get::<_, String>(0)?).map_err(|_| bad_row("id"))?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
    })
}

/// Record a failed delivery attempt. When `retry_at` is `None` the event is
/// terminally failed and will not be attempted again.
pub fn mark_event_attempt(
    store: &Store,
    id: uuid::Uuid,
    attempts: i64,
    retry_at: Option<DateTime<Utc>>,
) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        match retry_at {
            Some(at) => {
                c.execute(
                    "UPDATE events_outbox SET attempts = ?2, next_attempt_at = ?3 WHERE id = ?1",
                    rusqlite::params![id.to_string(), attempts, ts(at)],
                )
                .map_err(map_err)?;
            }
            None => {
                c.execute(
                    "UPDATE events_outbox SET attempts = ?2, failed_at = ?3, next_attempt_at = NULL WHERE id = ?1",
                    rusqlite::params![id.to_string(), attempts, ts(Utc::now())],
                )
                .map_err(map_err)?;
            }
        }
        Ok(())
    })
}

// --------------------------------------------------------------------------
// Webhooks (I-1)
// --------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct WebhookRow {
    pub id: WebhookId,
    pub url: String,
    pub secret: String,
    pub events: String,
    pub active: bool,
}

fn row_to_webhook(r: &Row<'_>) -> rusqlite::Result<WebhookRow> {
    Ok(WebhookRow {
        id: WebhookId::from_uuid(
            uuid::Uuid::parse_str(&r.get::<_, String>("id")?).map_err(|_| bad_row("id"))?,
        ),
        url: r.get("url")?,
        secret: r.get("secret")?,
        events: r.get("events")?,
        active: r.get::<_, i64>("active")? != 0,
    })
}

pub fn create_webhook(
    store: &Store,
    url: &str,
    secret: &str,
    events: &str,
) -> CoreResult<WebhookRow> {
    let row = WebhookRow {
        id: WebhookId::generate(),
        url: url.to_string(),
        secret: secret.to_string(),
        events: events.to_string(),
        active: true,
    };
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO webhooks (id, url, secret, events, active, created_at) VALUES (?1,?2,?3,?4,1,?5)",
            rusqlite::params![row.id.to_string(), row.url, row.secret, row.events, ts(Utc::now())],
        )
        .map_err(map_err)?;
        Ok(())
    })?;
    Ok(row)
}

pub fn list_active_webhooks(store: &Store) -> CoreResult<Vec<WebhookRow>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT * FROM webhooks WHERE active = 1")
            .map_err(map_err)?;
        let rows = stmt.query_map([], row_to_webhook).map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
    })
}

pub fn webhook_matches(row: &WebhookRow, event_type: &str) -> bool {
    row.events == "*" || row.events.split(',').any(|e| e.trim() == event_type)
}

// --------------------------------------------------------------------------
// Incidents (F-38)
// --------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize)]
pub struct IncidentRow {
    pub id: IncidentId,
    pub kind: String,
    pub severity: String,
    pub location: Option<String>,
    pub description: String,
    pub occurred_at: DateTime<Utc>,
    pub reported_by: String,
    pub status: String,
    pub actions: Vec<String>,
}

pub fn create_incident(store: &Store, i: &IncidentRow) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO incidents (id, kind, severity, location, description, occurred_at, reported_by, status, actions)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            rusqlite::params![
                i.id.to_string(), i.kind, i.severity, i.location, i.description,
                ts(i.occurred_at), i.reported_by, i.status,
                serde_json::to_string(&i.actions).map_err(|e| CoreError::Internal(e.to_string()))?
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn list_incidents(
    store: &Store,
    status: Option<&str>,
    limit: u32,
) -> CoreResult<Vec<IncidentRow>> {
    on_conn(&store.conn(), |c| {
        let (sql, param): (&str, Option<String>) = match status {
            Some(s) => (
                "SELECT * FROM incidents WHERE status = ?1 ORDER BY occurred_at DESC LIMIT ?2",
                Some(s.to_string()),
            ),
            None => (
                "SELECT * FROM incidents ORDER BY occurred_at DESC LIMIT ?1",
                None,
            ),
        };
        let mut stmt = c.prepare(sql).map_err(map_err)?;
        let rows = if let Some(p) = param {
            stmt.query_map(rusqlite::params![p, limit], row_to_incident)
        } else {
            stmt.query_map([limit], row_to_incident)
        }
        .map_err(map_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(map_err)
    })
}

fn row_to_incident(r: &Row<'_>) -> rusqlite::Result<IncidentRow> {
    Ok(IncidentRow {
        id: IncidentId::from_uuid(
            uuid::Uuid::parse_str(&r.get::<_, String>("id")?).map_err(|_| bad_row("id"))?,
        ),
        kind: r.get("kind")?,
        severity: r.get("severity")?,
        location: r.get("location")?,
        description: r.get("description")?,
        occurred_at: parse_ts(&r.get::<_, String>("occurred_at")?)
            .map_err(|_| bad_row("occurred_at"))?,
        reported_by: r.get("reported_by")?,
        status: r.get("status")?,
        actions: serde_json::from_str(&r.get::<_, String>("actions")?).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_outbox_webhooks_incidents() {
        let store = Store::open_in_memory().unwrap();

        append_audit(
            &store,
            &AuditRecord::system("gate.tx.start", "gate_transaction", "tx1"),
        )
        .unwrap();
        append_audit(
            &store,
            &AuditRecord::system("invoice.issue", "invoice", "INV-1"),
        )
        .unwrap();
        let audit = query_audit(&store, None, Some("gate_transaction"), None, 10).unwrap();
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].action, "gate.tx.start");

        let ev_id = uuid::Uuid::now_v7();
        append_event(
            &store,
            ev_id,
            "container.gate_in",
            r#"{"id":"1"}"#,
            Utc::now(),
        )
        .unwrap();
        let pending = pending_events(&store, 10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(due_events(&store, Utc::now(), 10).unwrap().len(), 1);
        // Backoff reschedules; terminal failure stops further attempts.
        mark_event_attempt(
            &store,
            ev_id,
            1,
            Some(Utc::now() + chrono::Duration::seconds(30)),
        )
        .unwrap();
        assert!(due_events(&store, Utc::now(), 10).unwrap().is_empty());
        mark_event_attempt(&store, ev_id, 5, None).unwrap();
        assert!(
            due_events(&store, Utc::now() + chrono::Duration::hours(1), 10)
                .unwrap()
                .is_empty()
        );
        mark_event_delivered(&store, ev_id).unwrap();
        assert!(pending_events(&store, 10).unwrap().is_empty());

        let wh = create_webhook(&store, "https://ops.example.com/hook", "s3cret", "*").unwrap();
        assert!(webhook_matches(&wh, "container.gate_in"));
        let wh2 = create_webhook(
            &store,
            "https://x.example.com",
            "s",
            "container.gate_out,invoice.issued",
        )
        .unwrap();
        assert!(webhook_matches(&wh2, "invoice.issued"));
        assert!(!webhook_matches(&wh2, "container.gate_in"));
        assert_eq!(list_active_webhooks(&store).unwrap().len(), 2);

        let incident = IncidentRow {
            id: IncidentId::generate(),
            kind: "near_miss".into(),
            severity: "minor".into(),
            location: Some("Yard A1".into()),
            description: "Truck-brake failure near pedestrian route".into(),
            occurred_at: Utc::now(),
            reported_by: "gate_clerk".into(),
            status: "open".into(),
            actions: vec!["route diverted".into()],
        };
        create_incident(&store, &incident).unwrap();
        assert_eq!(list_incidents(&store, Some("open"), 10).unwrap().len(), 1);
        bump_event_attempts(&store, uuid::Uuid::now_v7()).unwrap();
    }
}
