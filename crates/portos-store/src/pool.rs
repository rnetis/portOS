//! A small, purpose-built connection pool for SQLite.
//!
//! SQLite allows one writer; WAL mode permits concurrent readers. A pool of
//! independent connections with round-robin checkout is therefore more
//! effective than a queue, and keeps the store crate free of async
//! dependencies. Callers run work on the blocking thread pool (the API
//! wraps calls in `tokio::task::spawn_blocking`).

use portos_core::{CoreError, CoreResult};
use rusqlite::Connection;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub struct Pool {
    conns: Vec<Arc<Mutex<Connection>>>,
    counter: AtomicUsize,
}

impl Clone for Pool {
    fn clone(&self) -> Self {
        Pool {
            conns: self.conns.clone(),
            counter: AtomicUsize::new(self.counter.load(Ordering::Relaxed)),
        }
    }
}

/// Per-connection pragmas. `journal_mode` is persistent in the database
/// file, but `foreign_keys`, `busy_timeout` and `synchronous` are
/// per-connection and must be applied to every pooled connection.
fn configure_conn(conn: &Connection) -> CoreResult<()> {
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA synchronous = NORMAL;",
    )
    .map_err(|e| CoreError::Internal(format!("db configure: {e}")))?;
    Ok(())
}

impl Pool {
    /// Open `size` connections to a file-backed database.
    pub fn new(path: &Path, size: usize) -> CoreResult<Pool> {
        let mut conns = Vec::with_capacity(size);
        for i in 0..size.max(1) {
            let conn = Connection::open(path)
                .map_err(|e| CoreError::Internal(format!("open db conn {i}: {e}")))?;
            configure_conn(&conn)?;
            conns.push(Arc::new(Mutex::new(conn)));
        }
        Ok(Pool {
            conns,
            counter: AtomicUsize::new(0),
        })
    }

    /// Pool over one shared in-memory database (tests only).
    pub fn new_in_memory(size: usize) -> CoreResult<Pool> {
        let flags = rusqlite::OpenFlags::default()
            | rusqlite::OpenFlags::SQLITE_OPEN_URI
            | rusqlite::OpenFlags::SQLITE_OPEN_SHARED_CACHE;
        // Unique name per pool: parallel tests each get an isolated DB.
        let uri = format!(
            "file:portos_mem_{}?mode=memory&cache=shared",
            uuid::Uuid::now_v7().simple()
        );
        let mut conns = Vec::with_capacity(size.max(1));
        for _ in 0..size.max(1) {
            let conn = Connection::open_with_flags(uri.as_str(), flags)
                .map_err(|e| CoreError::Internal(format!("open mem db: {e}")))?;
            configure_conn(&conn)?;
            conns.push(Arc::new(Mutex::new(conn)));
        }
        Ok(Pool {
            conns,
            counter: AtomicUsize::new(0),
        })
    }

    /// Round-robin checkout. The returned guard is shared; SQLite handles
    /// serialization (busy_timeout is configured per connection).
    pub fn checkout(&self) -> Arc<Mutex<Connection>> {
        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % self.conns.len();
        self.conns[idx].clone()
    }

    #[cfg(test)]
    pub fn size(&self) -> usize {
        self.conns.len()
    }
}

/// Convenience: run `f` with a locked connection (mutable access).
pub fn with_conn<T>(
    conn: &Arc<Mutex<Connection>>,
    f: impl FnOnce(&mut Connection) -> CoreResult<T>,
) -> CoreResult<T> {
    let mut guard = conn
        .lock()
        .map_err(|_| CoreError::Internal("db mutex poisoned".into()))?;
    f(&mut guard)
}
