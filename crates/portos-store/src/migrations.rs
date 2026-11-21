//! Ordered, transactional migrations.

use portos_core::{CoreError, CoreResult};
use rusqlite::Connection;

const MIGRATIONS: &[(&str, &str)] = &[("0001_init", include_str!("migrations/0001_init.sql"))];

/// Apply all pending migrations, each in its own transaction.
pub fn run(conn: &mut Connection) -> CoreResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version TEXT PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now'))
        );",
    )
    .map_err(|e| CoreError::Internal(format!("migrations table: {e}")))?;

    let applied: Vec<String> = {
        let mut stmt = conn
            .prepare("SELECT version FROM schema_migrations")
            .map_err(|e| CoreError::Internal(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| CoreError::Internal(e.to_string()))?;
        rows.filter_map(|r| r.ok()).collect()
    };

    for (version, sql) in MIGRATIONS {
        if applied.iter().any(|v| v == version) {
            continue;
        }
        let tx = conn
            .transaction()
            .map_err(|e| CoreError::Internal(format!("migration tx: {e}")))?;
        tx.execute_batch(sql)
            .map_err(|e| CoreError::Internal(format!("migration {version}: {e}")))?;
        tx.execute("INSERT INTO schema_migrations (version) VALUES (?1)", [version])
            .map_err(|e| CoreError::Internal(format!("migration record: {e}")))?;
        tx.commit()
            .map_err(|e| CoreError::Internal(format!("migration commit: {e}")))?;
        tracing::info!(version, "migration applied");
    }
    Ok(())
}
