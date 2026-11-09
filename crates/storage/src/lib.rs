// SPDX-License-Identifier: AGPL-3.0-only
//! SQLite storage (single file, WAL). Secrets are stored only as envelope-
//! encrypted blobs (see `remotemusicrp-crypto`); the master key never touches
//! this database.
//!
//! The API is deliberately small and storage-agnostic so a Postgres backend
//! could implement the same operations later (not planned now).
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use remotemusicrp_crypto::envelope::blob_key_id;
use remotemusicrp_crypto::{Keyring, RecordContext, Zeroizing};
use rusqlite::{Connection, OptionalExtension, params};

mod migrations;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Db(String),
    #[error("crypto error: {0}")]
    Crypto(#[from] remotemusicrp_crypto::Error),
    #[error("database lock poisoned")]
    Poisoned,
    #[error("database schema is newer than this version of RemoteMusicRP")]
    TooNew,
    #[error("file error: {0:?}")]
    Io(std::io::ErrorKind),
}

impl From<rusqlite::Error> for StorageError {
    fn from(e: rusqlite::Error) -> Self {
        // rusqlite errors don't contain bound parameter values.
        Self::Db(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, StorageError>;

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Severity for activity-log entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub at: i64,
    pub level: String,
    pub kind: String,
    pub message: String,
}

/// Cached artwork lookup result (URLs only, never image bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedArtwork {
    pub source: Option<String>,
    pub image_url: Option<String>,
    pub link_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub fetched_at: i64,
}

/// A web UI session row. The raw token is never stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub token_hash: String,
    pub csrf: String,
    pub authenticated: bool,
    pub remember: bool,
    pub created_at: i64,
    pub last_seen: i64,
    pub expires_at: i64,
    /// Short non-secret description (e.g. browser family) for the UI.
    pub label: String,
}

const SECRETS_TABLE: &str = "secrets";
/// Row caps keep caches far below Last.fm's 100 MB storage limit.
const MAX_CACHE_ROWS: i64 = 5000;
const MAX_LOG_CHARS: usize = 500;

#[derive(Debug)]
pub struct Db {
    conn: Mutex<Connection>,
    path: PathBuf,
}

/// Create the database file with owner-only permissions before SQLite opens it.
fn precreate_private(path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| StorageError::Io(e.kind()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)
            .map_err(|e| StorageError::Io(e.kind()))?;
    }
    Ok(())
}

impl Db {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        precreate_private(&path)?;
        let conn = Connection::open(&path)?;
        Self::init(conn, path)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, PathBuf::from(":memory:"))
    }

    fn init(conn: Connection, path: PathBuf) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        // Overwrite deleted content so removed secrets don't linger in free pages.
        conn.pragma_update(None, "secure_delete", "ON")?;
        // Small page cache (~1 MiB) to keep RAM low.
        conn.pragma_update(None, "cache_size", -1024)?;
        conn.pragma_update(None, "journal_size_limit", 4 * 1024 * 1024)?;
        migrations::run(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = self.conn.lock().map_err(|_| StorageError::Poisoned)?;
        f(&conn)
    }

    // ---- settings -------------------------------------------------------

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        self.with(|c| {
            Ok(
                c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                    r.get(0)
                })
                .optional()?,
            )
        })
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO settings(key, value, updated_at) VALUES(?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![key, value, now_unix()],
            )?;
            Ok(())
        })
    }

    pub fn delete_setting(&self, key: &str) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM settings WHERE key = ?1", [key])?;
            Ok(())
        })
    }

    // ---- secrets (envelope-encrypted) -----------------------------------

    /// Encrypt and store a secret. `purpose` is bound as AAD and must be the
    /// same when reading.
    pub fn put_secret(
        &self,
        keyring: &Keyring,
        name: &str,
        purpose: &str,
        plaintext: &[u8],
    ) -> Result<()> {
        let ctx = RecordContext {
            table: SECRETS_TABLE,
            record_id: name,
            purpose,
        };
        let blob = keyring.seal(&ctx, plaintext)?;
        self.with(|c| {
            c.execute(
                "INSERT INTO secrets(name, purpose, blob, key_id, updated_at) VALUES(?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(name) DO UPDATE SET purpose = excluded.purpose, blob = excluded.blob,
                     key_id = excluded.key_id, updated_at = excluded.updated_at",
                params![name, purpose, blob, keyring.current_id(), now_unix()],
            )?;
            Ok(())
        })
    }

    pub fn get_secret(
        &self,
        keyring: &Keyring,
        name: &str,
        purpose: &str,
    ) -> Result<Option<Zeroizing<Vec<u8>>>> {
        let blob: Option<Vec<u8>> = self.with(|c| {
            Ok(
                c.query_row("SELECT blob FROM secrets WHERE name = ?1", [name], |r| {
                    r.get(0)
                })
                .optional()?,
            )
        })?;
        let Some(blob) = blob else { return Ok(None) };
        let ctx = RecordContext {
            table: SECRETS_TABLE,
            record_id: name,
            purpose,
        };
        Ok(Some(keyring.open(&ctx, &blob)?))
    }

    pub fn has_secret(&self, name: &str) -> Result<bool> {
        self.with(|c| {
            Ok(
                c.query_row("SELECT 1 FROM secrets WHERE name = ?1", [name], |_| Ok(()))
                    .optional()?
                    .is_some(),
            )
        })
    }

    pub fn delete_secret(&self, name: &str) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM secrets WHERE name = ?1", [name])?;
            Ok(())
        })
    }

    /// Delete all secrets whose name starts with `prefix` (e.g. disconnecting
    /// one integration).
    pub fn delete_secrets_with_prefix(&self, prefix: &str) -> Result<usize> {
        let pattern = format!(
            "{}%",
            prefix
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        self.with(|c| {
            Ok(c.execute(
                "DELETE FROM secrets WHERE name LIKE ?1 ESCAPE '\\'",
                [pattern],
            )?)
        })
    }

    /// Re-wrap every secret not yet using the keyring's current key. Returns
    /// the number of rows updated. Safe to run repeatedly (rotation job).
    pub fn rewrap_secrets(&self, keyring: &Keyring) -> Result<usize> {
        self.with(|c| {
            let tx = c.unchecked_transaction()?;
            let rows: Vec<(String, String, Vec<u8>)> = {
                let mut stmt = tx.prepare("SELECT name, purpose, blob FROM secrets WHERE key_id != ?1")?;
                let it = stmt.query_map([keyring.current_id()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                it.collect::<std::result::Result<_, _>>()?
            };
            let mut n = 0;
            for (name, purpose, blob) in rows {
                let ctx = RecordContext { table: SECRETS_TABLE, record_id: &name, purpose: &purpose };
                if let Some(new) = keyring.rewrap(&ctx, &blob)? {
                    let kid = blob_key_id(&new)?;
                    tx.execute(
                        "UPDATE secrets SET blob = ?1, key_id = ?2, updated_at = ?3 WHERE name = ?4",
                        params![new, kid, now_unix(), name],
                    )?;
                    n += 1;
                }
            }
            tx.commit()?;
            Ok(n)
        })
    }

    /// Key ids still referenced by stored secrets (old keys may be retired
    /// once only the current id remains).
    pub fn secret_key_ids(&self) -> Result<Vec<u32>> {
        self.with(|c| {
            let mut stmt = c.prepare("SELECT DISTINCT key_id FROM secrets ORDER BY key_id")?;
            let it = stmt.query_map([], |r| r.get::<_, u32>(0))?;
            Ok(it.collect::<std::result::Result<_, _>>()?)
        })
    }

    // ---- sessions -------------------------------------------------------

    pub fn insert_session(&self, row: &SessionRow) -> Result<()> {
        let label: String = row.label.chars().take(64).collect();
        self.with(|c| {
            c.execute(
                "INSERT INTO sessions(token_hash, csrf, authenticated, remember, created_at, last_seen, expires_at, label)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    row.token_hash,
                    row.csrf,
                    row.authenticated,
                    row.remember,
                    row.created_at,
                    row.last_seen,
                    row.expires_at,
                    label
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_session(&self, token_hash: &str) -> Result<Option<SessionRow>> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT token_hash, csrf, authenticated, remember, created_at, last_seen, expires_at, label
                 FROM sessions WHERE token_hash = ?1",
                [token_hash],
                session_from_row,
            )
            .optional()?)
        })
    }

    pub fn touch_session(&self, token_hash: &str, last_seen: i64) -> Result<()> {
        self.with(|c| {
            c.execute(
                "UPDATE sessions SET last_seen = ?1 WHERE token_hash = ?2",
                params![last_seen, token_hash],
            )?;
            Ok(())
        })
    }

    pub fn delete_session(&self, token_hash: &str) -> Result<()> {
        self.with(|c| {
            c.execute("DELETE FROM sessions WHERE token_hash = ?1", [token_hash])?;
            Ok(())
        })
    }

    /// Delete every session except `keep` (e.g. "log out other devices",
    /// password change). Returns the number deleted.
    pub fn delete_other_sessions(&self, keep: Option<&str>) -> Result<usize> {
        self.with(|c| {
            Ok(match keep {
                Some(k) => c.execute("DELETE FROM sessions WHERE token_hash != ?1", [k])?,
                None => c.execute("DELETE FROM sessions", [])?,
            })
        })
    }

    /// Authenticated sessions, most recent first.
    pub fn list_sessions(&self) -> Result<Vec<SessionRow>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT token_hash, csrf, authenticated, remember, created_at, last_seen, expires_at, label
                 FROM sessions WHERE authenticated = 1 ORDER BY last_seen DESC LIMIT 100",
            )?;
            let it = stmt.query_map([], session_from_row)?;
            Ok(it.collect::<std::result::Result<_, _>>()?)
        })
    }

    /// Keep only the newest `max` unauthenticated sessions.
    pub fn trim_anonymous_sessions(&self, max: i64) -> Result<usize> {
        self.with(|c| {
            Ok(c.execute(
                "DELETE FROM sessions WHERE authenticated = 0 AND token_hash NOT IN
                 (SELECT token_hash FROM sessions WHERE authenticated = 0 ORDER BY created_at DESC LIMIT ?1)",
                [max],
            )?)
        })
    }

    pub fn prune_sessions(&self, now: i64) -> Result<usize> {
        self.with(|c| Ok(c.execute("DELETE FROM sessions WHERE expires_at < ?1", [now])?))
    }

    // ---- activity log ---------------------------------------------------

    /// Append a log entry. Callers must never pass secrets; messages are also
    /// length-capped.
    pub fn log(&self, level: LogLevel, kind: &str, message: &str) -> Result<()> {
        let msg: String = message.chars().take(MAX_LOG_CHARS).collect();
        let kind: String = kind.chars().take(64).collect();
        self.with(|c| {
            c.execute(
                "INSERT INTO activity_log(at, level, kind, message) VALUES(?1, ?2, ?3, ?4)",
                params![now_unix(), level.as_str(), kind, msg],
            )?;
            Ok(())
        })
    }

    pub fn recent_log(&self, limit: u32) -> Result<Vec<LogEntry>> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT at, level, kind, message FROM activity_log ORDER BY id DESC LIMIT ?1",
            )?;
            let it = stmt.query_map([limit.min(1000)], |r| {
                Ok(LogEntry {
                    at: r.get(0)?,
                    level: r.get(1)?,
                    kind: r.get(2)?,
                    message: r.get(3)?,
                })
            })?;
            Ok(it.collect::<std::result::Result<_, _>>()?)
        })
    }

    /// Number of log entries per level since `since` (unix seconds):
    /// `(info, warn, error)`.
    pub fn log_counts_since(&self, since: i64) -> Result<(u64, u64, u64)> {
        self.with(|c| {
            let mut stmt = c.prepare(
                "SELECT level, COUNT(*) FROM activity_log WHERE at >= ?1 GROUP BY level",
            )?;
            let mut out = (0, 0, 0);
            let rows = stmt.query_map([since], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?;
            for row in rows {
                let (level, n) = row?;
                let n = u64::try_from(n).unwrap_or(0);
                match level.as_str() {
                    "info" => out.0 = n,
                    "warn" => out.1 = n,
                    "error" => out.2 = n,
                    _ => {}
                }
            }
            Ok(out)
        })
    }

    // ---- caches ---------------------------------------------------------

    pub fn get_artwork(&self, track_key: &str) -> Result<Option<CachedArtwork>> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT source, image_url, link_url, duration_ms, fetched_at FROM artwork_cache WHERE track_key = ?1",
                [track_key],
                |r| {
                    Ok(CachedArtwork {
                        source: r.get(0)?,
                        image_url: r.get(1)?,
                        link_url: r.get(2)?,
                        duration_ms: r.get(3)?,
                        fetched_at: r.get(4)?,
                    })
                },
            )
            .optional()?)
        })
    }

    pub fn put_artwork(&self, track_key: &str, art: &CachedArtwork) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO artwork_cache(track_key, source, image_url, link_url, duration_ms, fetched_at)
                 VALUES(?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(track_key) DO UPDATE SET source = excluded.source, image_url = excluded.image_url,
                     link_url = excluded.link_url, duration_ms = excluded.duration_ms, fetched_at = excluded.fetched_at",
                params![track_key, art.source, art.image_url, art.link_url, art.duration_ms, art.fetched_at],
            )?;
            Ok(())
        })
    }

    pub fn get_duration(&self, track_key: &str) -> Result<Option<Option<i64>>> {
        self.with(|c| {
            Ok(c.query_row(
                "SELECT duration_ms FROM track_info_cache WHERE track_key = ?1",
                [track_key],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()?)
        })
    }

    pub fn put_duration(&self, track_key: &str, duration_ms: Option<i64>) -> Result<()> {
        self.with(|c| {
            c.execute(
                "INSERT INTO track_info_cache(track_key, duration_ms, fetched_at) VALUES(?1, ?2, ?3)
                 ON CONFLICT(track_key) DO UPDATE SET duration_ms = excluded.duration_ms, fetched_at = excluded.fetched_at",
                params![track_key, duration_ms, now_unix()],
            )?;
            Ok(())
        })
    }

    /// Housekeeping: drop old log entries and trim caches. Run periodically.
    pub fn prune(&self, log_retention_days: u32) -> Result<()> {
        let cutoff = now_unix() - i64::from(log_retention_days) * 86_400;
        self.with(|c| {
            c.execute("DELETE FROM activity_log WHERE at < ?1", [cutoff])?;
            for table in ["artwork_cache", "track_info_cache"] {
                c.execute(
                    &format!(
                        "DELETE FROM {table} WHERE track_key NOT IN
                         (SELECT track_key FROM {table} ORDER BY fetched_at DESC LIMIT ?1)"
                    ),
                    [MAX_CACHE_ROWS],
                )?;
            }
            c.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(())
        })
    }

    /// Wipe all rows in place (used by "Delete everything" while running).
    /// With `secure_delete` on, freed pages are overwritten; VACUUM then
    /// rebuilds the file. The caller must also delete the master key.
    pub fn wipe(&self) -> Result<()> {
        self.with(|c| {
            c.execute_batch(
                "DELETE FROM secrets; DELETE FROM settings; DELETE FROM activity_log;
                 DELETE FROM artwork_cache; DELETE FROM track_info_cache; DELETE FROM sessions;
                 PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);",
            )?;
            Ok(())
        })
    }

    /// "Delete everything": close the database and remove its files. The
    /// caller must also delete the master key (crypto-shredding is the real
    /// guarantee; overwriting is not reliable on SSDs/CoW filesystems).
    pub fn destroy(self) -> Result<()> {
        let path = self.path.clone();
        {
            let conn = self.conn.into_inner().map_err(|_| StorageError::Poisoned)?;
            // Best effort: clear content so it is overwritten (secure_delete).
            let _ = conn.execute_batch(
                "DELETE FROM secrets; DELETE FROM settings; DELETE FROM activity_log;
                 DELETE FROM artwork_cache; DELETE FROM track_info_cache; DELETE FROM sessions;
                 PRAGMA wal_checkpoint(TRUNCATE); VACUUM;",
            );
            conn.close().map_err(|(_, e)| StorageError::from(e))?;
        }
        if path.as_os_str() == ":memory:" {
            return Ok(());
        }
        for suffix in ["", "-wal", "-shm"] {
            let mut p = path.clone().into_os_string();
            p.push(suffix);
            match std::fs::remove_file(PathBuf::from(p)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(StorageError::Io(e.kind())),
            }
        }
        Ok(())
    }
}

fn session_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        token_hash: r.get(0)?,
        csrf: r.get(1)?,
        authenticated: r.get(2)?,
        remember: r.get(3)?,
        created_at: r.get(4)?,
        last_seen: r.get(5)?,
        expires_at: r.get(6)?,
        label: r.get(7)?,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn session(hash: &str, auth: bool, expires: i64) -> SessionRow {
        SessionRow {
            token_hash: hash.into(),
            csrf: "c".into(),
            authenticated: auth,
            remember: false,
            created_at: 1,
            last_seen: 1,
            expires_at: expires,
            label: "Firefox".into(),
        }
    }

    #[test]
    fn sessions_crud() {
        let db = Db::open_in_memory().unwrap();
        db.insert_session(&session("a", true, 100)).unwrap();
        db.insert_session(&session("b", true, 200)).unwrap();
        db.insert_session(&session("anon", false, 200)).unwrap();
        assert!(db.get_session("a").unwrap().unwrap().authenticated);
        db.touch_session("a", 50).unwrap();
        assert_eq!(db.get_session("a").unwrap().unwrap().last_seen, 50);
        // Only authenticated sessions are listed.
        assert_eq!(db.list_sessions().unwrap().len(), 2);
        assert_eq!(db.prune_sessions(150).unwrap(), 1);
        assert!(db.get_session("a").unwrap().is_none());
        assert_eq!(db.delete_other_sessions(Some("b")).unwrap(), 1);
        assert!(db.get_session("b").unwrap().is_some());
        db.delete_session("b").unwrap();
        assert!(db.get_session("b").unwrap().is_none());
    }

    #[test]
    fn wipe_clears_everything() {
        let db = Db::open_in_memory().unwrap();
        let kr = Keyring::generate().unwrap();
        db.set_setting("a", "b").unwrap();
        db.put_secret(&kr, "s", "p", b"x").unwrap();
        db.insert_session(&session("a", true, 100)).unwrap();
        db.wipe().unwrap();
        assert!(db.get_setting("a").unwrap().is_none());
        assert!(!db.has_secret("s").unwrap());
        assert!(db.get_session("a").unwrap().is_none());
    }

    #[test]
    fn settings_roundtrip() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.get_setting("x").unwrap(), None);
        db.set_setting("x", "1").unwrap();
        db.set_setting("x", "2").unwrap();
        assert_eq!(db.get_setting("x").unwrap().as_deref(), Some("2"));
        db.delete_setting("x").unwrap();
        assert_eq!(db.get_setting("x").unwrap(), None);
    }

    #[test]
    fn secrets_are_encrypted_and_bound() {
        let db = Db::open_in_memory().unwrap();
        let kr = Keyring::generate().unwrap();
        db.put_secret(&kr, "lastfm.api_key", "api_key", b"0123456789abcdef")
            .unwrap();
        // Raw blob does not contain the plaintext.
        let raw: Vec<u8> = db
            .with(|c| Ok(c.query_row("SELECT blob FROM secrets", [], |r| r.get(0))?))
            .unwrap();
        assert!(!raw.windows(16).any(|w| w == b"0123456789abcdef"));
        assert_eq!(
            db.get_secret(&kr, "lastfm.api_key", "api_key")
                .unwrap()
                .unwrap()
                .as_slice(),
            b"0123456789abcdef"
        );
        // Wrong purpose fails authentication.
        assert!(db.get_secret(&kr, "lastfm.api_key", "other").is_err());
        assert!(db.has_secret("lastfm.api_key").unwrap());
        db.delete_secret("lastfm.api_key").unwrap();
        assert!(
            db.get_secret(&kr, "lastfm.api_key", "api_key")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn swapped_rows_fail() {
        let db = Db::open_in_memory().unwrap();
        let kr = Keyring::generate().unwrap();
        db.put_secret(&kr, "a", "p", b"secret-a").unwrap();
        db.put_secret(&kr, "b", "p", b"secret-b").unwrap();
        db.with(|c| {
            c.execute("UPDATE secrets SET blob = (SELECT blob FROM secrets WHERE name='b') WHERE name='a'", [])?;
            Ok(())
        })
        .unwrap();
        assert!(db.get_secret(&kr, "a", "p").is_err());
    }

    #[test]
    fn prefix_delete_escapes_wildcards() {
        let db = Db::open_in_memory().unwrap();
        let kr = Keyring::generate().unwrap();
        db.put_secret(&kr, "discord.1", "t", b"x").unwrap();
        db.put_secret(&kr, "discord_2", "t", b"x").unwrap();
        db.put_secret(&kr, "lastfm", "t", b"x").unwrap();
        assert_eq!(db.delete_secrets_with_prefix("discord.").unwrap(), 1);
        assert!(db.has_secret("discord_2").unwrap());
        assert!(db.has_secret("lastfm").unwrap());
    }

    #[test]
    fn rotation_rewraps_all() {
        let db = Db::open_in_memory().unwrap();
        let mut kr = Keyring::generate().unwrap();
        db.put_secret(&kr, "a", "p", b"1").unwrap();
        db.put_secret(&kr, "b", "q", b"2").unwrap();
        kr.rotate().unwrap();
        assert_eq!(db.rewrap_secrets(&kr).unwrap(), 2);
        assert_eq!(db.rewrap_secrets(&kr).unwrap(), 0);
        assert_eq!(db.secret_key_ids().unwrap(), vec![2]);
        kr.retire_old_keys();
        assert_eq!(
            db.get_secret(&kr, "b", "q").unwrap().unwrap().as_slice(),
            b"2"
        );
    }

    #[test]
    fn log_is_capped_and_ordered() {
        let db = Db::open_in_memory().unwrap();
        db.log(LogLevel::Info, "presence", "first").unwrap();
        db.log(LogLevel::Warn, "lastfm", &"x".repeat(2000)).unwrap();
        let entries = db.recent_log(10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].level, "warn");
        assert_eq!(entries[0].message.chars().count(), MAX_LOG_CHARS);
        assert_eq!(entries[1].message, "first");
    }

    #[test]
    fn caches_roundtrip_and_prune() {
        let db = Db::open_in_memory().unwrap();
        let art = CachedArtwork {
            source: Some("apple".into()),
            image_url: Some("https://is1-ssl.mzstatic.com/x.jpg".into()),
            link_url: None,
            duration_ms: Some(1000),
            fetched_at: now_unix(),
        };
        db.put_artwork("k", &art).unwrap();
        assert_eq!(db.get_artwork("k").unwrap().unwrap(), art);
        db.put_duration("k", None).unwrap();
        assert_eq!(db.get_duration("k").unwrap(), Some(None));
        assert_eq!(db.get_duration("missing").unwrap(), None);
        db.prune(30).unwrap();
        assert!(db.get_artwork("k").unwrap().is_some());
    }

    #[test]
    fn file_db_and_destroy() {
        let dir =
            std::env::temp_dir().join(format!("sa-db-test-{}-{}", std::process::id(), now_unix()));
        let path = dir.join("remotemusicrp.db");
        let db = Db::open(&path).unwrap();
        db.set_setting("a", "b").unwrap();
        assert!(path.exists());
        db.destroy().unwrap();
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reopen_keeps_data() {
        let dir = std::env::temp_dir().join(format!(
            "sa-db-reopen-{}-{}",
            std::process::id(),
            now_unix()
        ));
        let path = dir.join("db.sqlite");
        {
            let db = Db::open(&path).unwrap();
            db.set_setting("k", "v").unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.get_setting("k").unwrap().as_deref(), Some("v"));
        db.destroy().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
