//! Holler local store (Phase 1): a searchable SQLite history of transcripts.
//!
//! Clipboard handling lives in the app (it's main-thread, ephemeral output);
//! this crate is pure persistence so it's testable without a display.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use rusqlite::{params, Connection};

#[derive(Debug)]
pub enum StoreError {
    /// Could not determine or create the data directory.
    Path(String),
    /// A database operation failed.
    Db(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Path(m) => write!(f, "data path error: {m}"),
            StoreError::Db(m) => write!(f, "history database error: {m}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// One stored transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: i64,
    pub text: String,
    pub provider: String,
    /// Unix epoch seconds.
    pub created_at: i64,
}

/// The transcript history, backed by SQLite. `Connection` is `Send` but not
/// `Sync`; keep one `History` per thread (Holler uses it from the main thread).
pub struct History {
    conn: Connection,
}

impl History {
    /// Open (creating if needed) the history DB at the default app data path:
    /// `<data_dir>/Holler/history.db`.
    pub fn open_default() -> Result<Self, StoreError> {
        Self::open(&default_db_path()?)
    }

    /// Open (creating if needed) the history DB at `path`.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Path(e.to_string()))?;
        }
        let conn = Connection::open(path).map_err(|e| StoreError::Db(e.to_string()))?;
        Self::from_connection(conn)
    }

    /// In-memory database — used by tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory().map_err(|e| StoreError::Db(e.to_string()))?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self, StoreError> {
        // Briefly retry on a locked DB (a second instance, a backup/sync tool)
        // rather than instantly failing — a dropped transcript would be lost.
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| StoreError::Db(e.to_string()))?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS transcripts (
                id         INTEGER PRIMARY KEY,
                text       TEXT NOT NULL,
                provider   TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
            [],
        )
        .map_err(|e| StoreError::Db(e.to_string()))?;
        Ok(Self { conn })
    }

    /// Record a transcript; returns its new row id.
    pub fn record(&self, text: &str, provider: &str) -> Result<i64, StoreError> {
        self.conn
            .execute(
                "INSERT INTO transcripts (text, provider, created_at) VALUES (?1, ?2, ?3)",
                params![text, provider, now_unix()],
            )
            .map_err(|e| StoreError::Db(e.to_string()))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Full-text-ish search (substring, case-insensitive), newest first.
    pub fn search(&self, query: &str) -> Result<Vec<Entry>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, text, provider, created_at FROM transcripts
                 WHERE text LIKE ?1 ORDER BY created_at DESC, id DESC",
            )
            .map_err(|e| StoreError::Db(e.to_string()))?;
        let pattern = format!("%{query}%");
        let rows = stmt
            .query_map(params![pattern], |row| {
                Ok(Entry {
                    id: row.get(0)?,
                    text: row.get(1)?,
                    provider: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| StoreError::Db(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Db(e.to_string()))
    }

    /// The most recent `limit` transcripts, newest first.
    pub fn recent(&self, limit: usize) -> Result<Vec<Entry>, StoreError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, text, provider, created_at FROM transcripts
                 ORDER BY created_at DESC, id DESC LIMIT ?1",
            )
            .map_err(|e| StoreError::Db(e.to_string()))?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(Entry {
                    id: row.get(0)?,
                    text: row.get(1)?,
                    provider: row.get(2)?,
                    created_at: row.get(3)?,
                })
            })
            .map_err(|e| StoreError::Db(e.to_string()))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| StoreError::Db(e.to_string()))
    }
}

/// `<data_dir>/Holler/history.db` (macOS `~/Library/Application Support`,
/// Linux `~/.local/share`, Windows `%APPDATA%`).
pub fn default_db_path() -> Result<PathBuf, StoreError> {
    let dirs = ProjectDirs::from("com", "Holler", "Holler")
        .ok_or_else(|| StoreError::Path("could not determine a data directory".into()))?;
    Ok(dirs.data_dir().join("history.db"))
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_search_roundtrip() {
        let h = History::open_in_memory().unwrap();
        h.record("Please review the PR", "deepgram").unwrap();
        h.record("Hello there", "openai").unwrap();

        let hits = h.search("review").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "Please review the PR");
        assert_eq!(hits[0].provider, "deepgram");

        // Case-insensitive substring.
        assert_eq!(h.search("HELLO").unwrap().len(), 1);
        assert_eq!(h.search("nomatch").unwrap().len(), 0);
    }

    #[test]
    fn recent_returns_newest_first_within_limit() {
        let h = History::open_in_memory().unwrap();
        for i in 0..5 {
            h.record(&format!("entry {i}"), "deepgram").unwrap();
        }
        let recent = h.recent(3).unwrap();
        assert_eq!(recent.len(), 3);
        // Newest (highest id) first.
        assert_eq!(recent[0].text, "entry 4");
    }
}
