//! `app-db` — SQLite WAL + FTS5 catalog (CLAUDE.md §5).
//!
//! * PRAGMA set applied on **every** connection ([`apply_pragmas`]).
//! * Migrations embedded from `/migrations/*.sql`, tracked with `PRAGMA user_version`.
//! * One writer connection + a small pool of readers. WAL lets readers proceed during imports.
//! * Names are normalized at write time ([`normalize::normalize_name`]).
//! * Inserts are chunked, max 5,000 rows per transaction.

pub mod channels;
pub mod epg;
pub mod normalize;
pub mod progress;
pub mod recordings;
pub mod search;
pub mod settings;
pub mod vod;

use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub use rusqlite;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration {0} failed: {1}")]
    Migration(&'static str, rusqlite::Error),
    #[error("lock poisoned")]
    Poisoned,
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, DbError>;

/// Embedded migrations, applied in order. Add new files here; never edit shipped ones.
pub const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_initial", include_str!("../../../migrations/0001_initial.sql")),
    ("0002_epg_vod_recording", include_str!("../../../migrations/0002_epg_vod_recording.sql")),
];

pub const MAX_ROWS_PER_TX: usize = 5_000;

pub struct Db {
    path: PathBuf,
    writer: Mutex<Connection>,
    readers: Mutex<Vec<Connection>>,
    /// Parental keyword filter (lowercase). When non-empty, list/search queries exclude rows whose
    /// name/title or group/category contains any keyword.
    hidden: std::sync::RwLock<Vec<String>>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db").field("path", &self.path).finish()
    }
}

/// Apply the mandatory PRAGMA set. Called for every connection we open.
pub fn apply_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA temp_store = MEMORY;
        PRAGMA cache_size = -64000;
        PRAGMA mmap_size = 268435456;
        PRAGMA busy_timeout = 5000;
        "#,
    )
}

fn open_conn(path: &Path) -> rusqlite::Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX
        | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(path, flags)?;
    apply_pragmas(&conn)?;
    Ok(conn)
}

impl Db {
    /// Open (or create) the catalog at `path` and run pending migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| DbError::Other(e.to_string()))?;
            }
        }
        let writer = open_conn(&path)?;
        let db = Self { path, writer: Mutex::new(writer), readers: Mutex::new(Vec::new()), hidden: Default::default() };
        db.migrate()?;
        Ok(db)
    }

    /// In-memory database for tests (shared-cache URI so reader connections see the same data).
    pub fn open_in_memory() -> Result<Self> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let uri = format!("file:memdb{}_{}?mode=memory&cache=shared", std::process::id(), n);
        let writer = open_conn(Path::new(&uri))?;
        let db = Self {
            path: PathBuf::from(uri),
            writer: Mutex::new(writer),
            readers: Mutex::new(Vec::new()),
            hidden: Default::default(),
        };
        db.migrate()?;
        Ok(db)
    }

    /// Set the parental keyword filter applied to catalog queries (empty = show everything).
    pub fn set_hidden_keywords(&self, keywords: &[String]) {
        let mut kws: Vec<String> = keywords.iter().map(|k| k.trim().to_lowercase()).filter(|k| !k.is_empty()).collect();
        kws.sort();
        kws.dedup();
        *self.hidden.write().unwrap() = kws;
    }

    pub fn hidden_keywords(&self) -> Vec<String> {
        self.hidden.read().unwrap().clone()
    }

    /// SQL fragment (starting with " AND ") that excludes rows matching the hidden keywords, or
    /// an empty string. Keywords are embedded as escaped SQL string literals (they are lowercased
    /// and single quotes doubled), never interpolated raw.
    pub fn hidden_clause(&self, cols: &[&str]) -> String {
        let kws = self.hidden.read().unwrap();
        if kws.is_empty() || cols.is_empty() {
            return String::new();
        }
        let mut parts = Vec::new();
        for kw in kws.iter() {
            let lit = kw.replace('\'', "''");
            for c in cols {
                parts.push(format!("instr(lower(COALESCE({c}, '')), '{lit}') > 0"));
            }
        }
        format!(" AND NOT ({})", parts.join(" OR "))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.writer.lock().map_err(|_| DbError::Poisoned)?;
        let current: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        for (i, (name, sql)) in MIGRATIONS.iter().enumerate() {
            let version = (i + 1) as i64;
            if version <= current {
                continue;
            }
            tracing::info!(migration = name, "applying");
            conn.execute_batch("BEGIN").map_err(|e| DbError::Migration(name, e))?;
            if let Err(e) = conn.execute_batch(sql) {
                let _ = conn.execute_batch("ROLLBACK");
                return Err(DbError::Migration(name, e));
            }
            conn.pragma_update(None, "user_version", version).map_err(|e| DbError::Migration(name, e))?;
            conn.execute_batch("COMMIT").map_err(|e| DbError::Migration(name, e))?;
        }
        Ok(())
    }

    /// Run `f` with the single writer connection.
    pub fn with_write<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut conn = self.writer.lock().map_err(|_| DbError::Poisoned)?;
        f(&mut conn)
    }

    /// Run `f` with a pooled reader connection (opened lazily, max 4 kept).
    pub fn with_read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let conn = {
            let mut pool = self.readers.lock().map_err(|_| DbError::Poisoned)?;
            pool.pop()
        };
        let conn = match conn {
            Some(c) => c,
            None => open_conn(&self.path)?,
        };
        let out = f(&conn);
        let mut pool = self.readers.lock().map_err(|_| DbError::Poisoned)?;
        if pool.len() < 4 {
            pool.push(conn);
        }
        out
    }

    /// Run a WAL checkpoint (call after big imports so readers don't pay for a huge WAL).
    pub fn checkpoint(&self) -> Result<()> {
        self.with_write(|c| {
            c.execute_batch("PRAGMA wal_checkpoint(PASSIVE)")?;
            Ok(())
        })
    }

    pub fn schema_version(&self) -> Result<i64> {
        self.with_read(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get(0))?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_and_migrates() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), MIGRATIONS.len() as i64);
        let tables: Vec<String> = db
            .with_read(|c| {
                let mut st = c.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
            })
            .unwrap();
        for t in ["playlists", "channels", "vod_items", "epg_programmes", "settings", "search_idx", "recents"] {
            assert!(tables.iter().any(|x| x == t), "missing table {t}: {tables:?}");
        }
        // migrate is idempotent
        db.migrate().unwrap();
    }

    #[test]
    fn pragmas_apply_on_file_db() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path().join("cat.db")).unwrap();
        let mode: String = db.with_read(|c| Ok(c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?)).unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let fk: i64 = db.with_read(|c| Ok(c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?)).unwrap();
        assert_eq!(fk, 1);
    }
}
