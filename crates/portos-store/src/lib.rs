//! PortOS persistence layer.
//!
//! SQLite in WAL mode with an ordered, transactional migration runner and
//! repository modules grouped by bounded context. Connections are managed
//! by a small hand-rolled pool and every call runs on the blocking thread
//! pool, keeping the async API surface clean.

pub mod billing;
pub mod fleet;
pub mod gate;
pub mod migrations;
pub mod pool;
pub mod users;
pub mod util;
pub mod work;
pub mod yard;
pub mod misc;

use portos_core::{CoreError, CoreResult};
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

pub use pool::Pool;

/// Entry point to all repositories. Cloneable (shares the pool).
#[derive(Clone)]
pub struct Store {
    pub pool: Pool,
}

/// Lock a pooled connection, mapping poisoning to an internal error.
pub fn lock_conn(
    conn: &Arc<Mutex<Connection>>,
) -> CoreResult<std::sync::MutexGuard<'_, Connection>> {
    conn.lock()
        .map_err(|_| CoreError::Internal("db mutex poisoned".into()))
}

impl Store {
    /// Open (creating parent directories as needed), configure pragmas and
    /// run pending migrations.
    pub fn open(path: &Path) -> CoreResult<Store> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CoreError::Internal(format!("create db dir: {e}")))?;
            }
        }
        let pool = Pool::new(path, 8)?;
        {
            let conn = pool.checkout();
            let mut guard = lock_conn(&conn)?;
            Self::configure(&guard)?;
            migrations::run(&mut guard)?;
        }
        Ok(Store { pool })
    }

    /// Open an in-memory database (tests).
    pub fn open_in_memory() -> CoreResult<Store> {
        let pool = Pool::new_in_memory(4)?;
        {
            let conn = pool.checkout();
            let mut guard = lock_conn(&conn)?;
            migrations::run(&mut guard)?;
        }
        Ok(Store { pool })
    }

    fn configure(conn: &Connection) -> CoreResult<()> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;
             PRAGMA synchronous = NORMAL;",
        )
        .map_err(|e| CoreError::Internal(format!("db configure: {e}")))?;
        Ok(())
    }

    /// Borrow one pooled connection for ad-hoc work.
    pub fn conn(&self) -> Arc<std::sync::Mutex<Connection>> {
        self.pool.checkout()
    }
}
