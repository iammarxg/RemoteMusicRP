// SPDX-License-Identifier: AGPL-3.0-only
//! Forward-only schema migrations tracked with `PRAGMA user_version`.

use rusqlite::Connection;

use crate::{Result, StorageError};

const MIGRATIONS: &[&str] = &[
    // v1: initial schema
    "CREATE TABLE settings (
         key        TEXT PRIMARY KEY NOT NULL,
         value      TEXT NOT NULL,
         updated_at INTEGER NOT NULL
     ) STRICT;
     CREATE TABLE secrets (
         name       TEXT PRIMARY KEY NOT NULL,
         purpose    TEXT NOT NULL,
         blob       BLOB NOT NULL,
         key_id     INTEGER NOT NULL,
         updated_at INTEGER NOT NULL
     ) STRICT;
     CREATE TABLE activity_log (
         id      INTEGER PRIMARY KEY AUTOINCREMENT,
         at      INTEGER NOT NULL,
         level   TEXT NOT NULL,
         kind    TEXT NOT NULL,
         message TEXT NOT NULL
     ) STRICT;
     CREATE INDEX activity_log_at ON activity_log(at);
     CREATE TABLE artwork_cache (
         track_key   TEXT PRIMARY KEY NOT NULL,
         source      TEXT,
         image_url   TEXT,
         link_url    TEXT,
         duration_ms INTEGER,
         fetched_at  INTEGER NOT NULL
     ) STRICT;
     CREATE TABLE track_info_cache (
         track_key   TEXT PRIMARY KEY NOT NULL,
         duration_ms INTEGER,
         fetched_at  INTEGER NOT NULL
     ) STRICT;",
    // v2: web UI sessions (token stored only as SHA-256 hash)
    "CREATE TABLE sessions (
         token_hash    TEXT PRIMARY KEY NOT NULL,
         csrf          TEXT NOT NULL,
         authenticated INTEGER NOT NULL,
         remember      INTEGER NOT NULL,
         created_at    INTEGER NOT NULL,
         last_seen     INTEGER NOT NULL,
         expires_at    INTEGER NOT NULL,
         label         TEXT NOT NULL
     ) STRICT;
     CREATE INDEX sessions_expires ON sessions(expires_at);",
];

pub(crate) fn run(conn: &Connection) -> Result<()> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let target = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
    if current > target {
        return Err(StorageError::TooNew);
    }
    for (i, sql) in MIGRATIONS
        .iter()
        .enumerate()
        .skip(usize::try_from(current).unwrap_or(0))
    {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(
            None,
            "user_version",
            i64::try_from(i + 1).unwrap_or(i64::MAX),
        )?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn migrations_idempotent_and_reject_newer() {
        let c = Connection::open_in_memory().unwrap();
        run(&c).unwrap();
        run(&c).unwrap();
        c.pragma_update(None, "user_version", 999).unwrap();
        assert!(matches!(run(&c), Err(StorageError::TooNew)));
    }
}
