//! User accounts and saved localization preferences (PRD L-1, F-44).

use crate::util::{map_err, on_conn, ts};
use crate::Store;
use chrono::{DateTime, Utc};
use portos_core::ids::UserId;
use portos_core::{CoreError, CoreResult};
use portos_i18n::{Calendar, Locale, Numerals};
use rusqlite::Connection;

/// A user row. The password hash is an Argon2 PHC string — never stored or
/// logged in plaintext (PRD §6.3).
#[derive(Debug, Clone, serde::Serialize)]
pub struct UserRow {
    pub id: UserId,
    pub username: String,
    pub password_hash: String,
    pub display_name: String,
    pub role: String,
    pub locale: Locale,
    pub calendar: Calendar,
    pub numerals: Numerals,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn bad_row(col: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(
        0,
        col.to_string(),
        rusqlite::types::Type::Text,
    )
}

fn row_ts(r: &rusqlite::Row<'_>, col: &str) -> rusqlite::Result<chrono::DateTime<chrono::Utc>> {
    let s: String = r.get(col)?;
    crate::util::parse_ts(&s).map_err(|_| bad_row(col))
}

fn row_to_user(r: &rusqlite::Row<'_>) -> rusqlite::Result<UserRow> {
    let locale: String = r.get("locale")?;
    let calendar: String = r.get("calendar")?;
    let numerals: String = r.get("numerals")?;
    let id_str: String = r.get("id")?;
    let uuid = uuid::Uuid::parse_str(&id_str).map_err(|_| bad_row("id"))?;
    Ok(UserRow {
        id: UserId::from_uuid(uuid),
        username: r.get("username")?,
        password_hash: r.get("password_hash")?,
        display_name: r.get("display_name")?,
        role: r.get("role")?,
        locale: Locale::parse(&locale).unwrap_or_default(),
        calendar: Calendar::parse(&calendar).unwrap_or_default(),
        numerals: Numerals::parse(&numerals).unwrap_or_default(),
        active: r.get::<_, i64>("active")? != 0,
        created_at: row_ts(r, "created_at")?,
        updated_at: row_ts(r, "updated_at")?,
    })
}

pub fn create(
    store: &Store,
    username: &str,
    password_hash: &str,
    display_name: &str,
    role: &str,
    locale: Locale,
    calendar: Calendar,
    numerals: Numerals,
) -> CoreResult<UserRow> {
    let id = UserId::generate();
    let now = ts(Utc::now());
    on_conn(&store.conn(), |c| {
        c.execute(
            "INSERT INTO users (id, username, password_hash, display_name, role, locale, calendar, numerals, active, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9)",
            rusqlite::params![
                id.to_string(),
                username,
                password_hash,
                display_name,
                role,
                locale.tag(),
                match calendar { Calendar::Gregorian => "gregorian", Calendar::Jalali => "jalali" },
                match numerals { Numerals::Western => "western", Numerals::Persian => "persian" },
                now
            ],
        )
        .map_err(map_err)?;
        Ok(())
    })?;
    find_by_username(store, username)?
        .ok_or_else(|| CoreError::Internal("user vanished after insert".into()))
}

pub fn find_by_username(store: &Store, username: &str) -> CoreResult<Option<UserRow>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT * FROM users WHERE username = ?1")
            .map_err(map_err)?;
        let mut rows = stmt.query_map([username], row_to_user).map_err(map_err)?;
        match rows.next() {
            Some(row) => Ok(Some(row.map_err(map_err)?)),
            None => Ok(None),
        }
    })
}

pub fn find_by_id(store: &Store, id: UserId) -> CoreResult<Option<UserRow>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c
            .prepare("SELECT * FROM users WHERE id = ?1")
            .map_err(map_err)?;
        let mut rows = stmt.query_map([id.to_string()], row_to_user).map_err(map_err)?;
        match rows.next() {
            Some(row) => Ok(Some(row.map_err(map_err)?)),
            None => Ok(None),
        }
    })
}

pub fn update_preferences(
    store: &Store,
    id: UserId,
    locale: Option<Locale>,
    calendar: Option<Calendar>,
    numerals: Option<Numerals>,
) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        if let Some(l) = locale {
            c.execute("UPDATE users SET locale = ?2, updated_at = ?3 WHERE id = ?1", rusqlite::params![id.to_string(), l.tag(), ts(Utc::now())])
                .map_err(map_err)?;
        }
        if let Some(cal) = calendar {
            let v = match cal { Calendar::Gregorian => "gregorian", Calendar::Jalali => "jalali" };
            c.execute("UPDATE users SET calendar = ?2, updated_at = ?3 WHERE id = ?1", rusqlite::params![id.to_string(), v, ts(Utc::now())])
                .map_err(map_err)?;
        }
        if let Some(n) = numerals {
            let v = match n { Numerals::Western => "western", Numerals::Persian => "persian" };
            c.execute("UPDATE users SET numerals = ?2, updated_at = ?3 WHERE id = ?1", rusqlite::params![id.to_string(), v, ts(Utc::now())])
                .map_err(map_err)?;
        }
        Ok(())
    })
}

pub fn set_active(store: &Store, id: UserId, active: bool) -> CoreResult<()> {
    on_conn(&store.conn(), |c| {
        c.execute(
            "UPDATE users SET active = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id.to_string(), i64::from(active), ts(Utc::now())],
        )
        .map_err(map_err)?;
        Ok(())
    })
}

pub fn list(store: &Store) -> CoreResult<Vec<UserRow>> {
    on_conn(&store.conn(), |c| {
        let mut stmt = c.prepare("SELECT * FROM users ORDER BY username").map_err(map_err)?;
        let rows = stmt.query_map([], row_to_user).map_err(map_err)?;
        rows.filter_map(|r| r.ok()).map(Ok).collect::<CoreResult<Vec<_>>>()
    })
}

pub fn count(store: &Store, conn: Option<&Connection>) -> CoreResult<i64> {
    match conn {
        Some(c) => {
            let n: i64 = c
                .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
                .map_err(map_err)?;
            Ok(n)
        }
        None => on_conn(&store.conn(), |c| count(store, Some(c))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crud_and_prefs() {
        let store = Store::open_in_memory().unwrap();
        let u = create(
            &store,
            "planner",
            "$argon2id$hash",
            "پلانر بندر",
            "ops_planner",
            Locale::Fa,
            Calendar::Jalali,
            Numerals::Persian,
        )
        .unwrap();
        assert_eq!(u.locale, Locale::Fa);
        let got = find_by_username(&store, "planner").unwrap().unwrap();
        assert_eq!(got.id, u.id);
        update_preferences(&store, u.id, Some(Locale::En), None, Some(Numerals::Western)).unwrap();
        let got = find_by_id(&store, u.id).unwrap().unwrap();
        assert_eq!(got.locale, Locale::En);
        assert_eq!(got.numerals, Numerals::Western);
        assert!(find_by_username(&store, "nope").unwrap().is_none());
        assert!(count(&store, None).unwrap() >= 1);
    }

    #[test]
    fn duplicate_username_conflicts() {
        let store = Store::open_in_memory().unwrap();
        create(&store, "admin", "h", "Admin", "admin", Locale::En, Calendar::Gregorian, Numerals::Western).unwrap();
        let second = create(&store, "admin", "h", "Admin2", "admin", Locale::En, Calendar::Gregorian, Numerals::Western);
        assert!(matches!(second, Err(CoreError::Conflict(_))));
    }
}
