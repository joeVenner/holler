//! Holler local store (Phase 1): a searchable SQLite history of transcripts.
//!
//! Clipboard handling lives in the app (it's main-thread, ephemeral output);
//! this crate is pure persistence so it's testable without a display.

use std::collections::BTreeMap;
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

/// A snapshot of local usage, computed on demand from the history rows.
/// All counts are honest (no telemetry, no estimation) — the recency windows
/// are rolling deltas from "now", not calendar days, so they need no timezone
/// data and behave identically on macOS and Windows (see DISCOVERIES).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stats {
    /// Total transcripts ever recorded.
    pub total: u64,
    /// Total words across every transcript (whitespace-split).
    pub words: u64,
    /// `(provider id, count)` pairs, busiest first (ties broken by id).
    pub by_provider: Vec<(String, u64)>,
    /// Transcripts whose age is under 24 hours.
    pub last_24h: u64,
    /// Transcripts whose age is under 7 days.
    pub last_7d: u64,
}

impl Stats {
    /// Mean words per transcript (0.0 when there are none).
    pub fn avg_words(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.words as f64 / self.total as f64
        }
    }
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

    /// Delete the transcript with `id`. Idempotent — deleting a row that no
    /// longer exists (e.g. a stale GUI list) is not an error.
    pub fn delete(&self, id: i64) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM transcripts WHERE id = ?1", params![id])
            .map_err(|e| StoreError::Db(e.to_string()))?;
        Ok(())
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

    /// Aggregate local usage statistics relative to `now` (Unix epoch seconds).
    /// Word counts can't be done in SQL, so this scans every row once in Rust —
    /// sub-millisecond for a personal dictation history, and it keeps the maths
    /// honest and testable (pass a fixed `now`).
    pub fn stats(&self, now: i64) -> Result<Stats, StoreError> {
        const DAY: i64 = 86_400;
        let mut stmt = self
            .conn
            .prepare("SELECT text, provider, created_at FROM transcripts")
            .map_err(|e| StoreError::Db(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|e| StoreError::Db(e.to_string()))?;

        let mut stats = Stats::default();
        let mut by_provider: BTreeMap<String, u64> = BTreeMap::new();
        for row in rows {
            let (text, provider, created_at) = row.map_err(|e| StoreError::Db(e.to_string()))?;
            stats.total += 1;
            stats.words += text.split_whitespace().count() as u64;
            *by_provider.entry(provider).or_insert(0) += 1;
            // A future-dated row (clock skew) reads as age 0 — recent, so it
            // counts toward both windows rather than silently vanishing.
            let age = now - created_at;
            if age < DAY {
                stats.last_24h += 1;
            }
            if age < 7 * DAY {
                stats.last_7d += 1;
            }
        }

        // Busiest provider first; ties fall back to id order for a stable list.
        let mut by_provider: Vec<(String, u64)> = by_provider.into_iter().collect();
        by_provider.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        stats.by_provider = by_provider;
        Ok(stats)
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

    #[test]
    fn delete_removes_only_the_target_and_is_idempotent() {
        let h = History::open_in_memory().unwrap();
        let keep = h.record("keep me", "deepgram").unwrap();
        let drop = h.record("delete me", "openai").unwrap();

        h.delete(drop).unwrap();
        let left = h.recent(10).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].id, keep);
        assert_eq!(left[0].text, "keep me");

        // Deleting an already-gone row is a no-op, not an error.
        h.delete(drop).unwrap();
        assert_eq!(h.recent(10).unwrap().len(), 1);
    }

    #[test]
    fn stats_over_empty_db_are_all_zero() {
        let h = History::open_in_memory().unwrap();
        let s = h.stats(1_000_000).unwrap();
        assert_eq!(s, Stats::default());
        assert_eq!(s.avg_words(), 0.0);
    }

    #[test]
    fn stats_aggregate_words_providers_and_recency_windows() {
        let h = History::open_in_memory().unwrap();
        // Insert rows with explicit timestamps so the rolling windows are
        // deterministic (record() stamps "now", so write created_at directly).
        let now = 10 * 86_400; // 10 days past the epoch
        let insert = |text: &str, provider: &str, created_at: i64| {
            h.conn
                .execute(
                    "INSERT INTO transcripts (text, provider, created_at) VALUES (?1, ?2, ?3)",
                    params![text, provider, created_at],
                )
                .unwrap();
        };
        insert("one two three", "deepgram", now - 100); // <24h, 3 words
        insert("hello world", "openai", now - 3 * 86_400); // 3 days, 2 words
        insert("alpha beta gamma delta", "deepgram", now - 8 * 86_400); // 8 days, 4 words

        let s = h.stats(now).unwrap();
        assert_eq!(s.total, 3);
        assert_eq!(s.words, 9);
        assert_eq!(s.last_24h, 1);
        assert_eq!(s.last_7d, 2);
        assert_eq!(s.avg_words(), 3.0);
        // Busiest provider first: deepgram (2) before openai (1).
        assert_eq!(
            s.by_provider,
            vec![("deepgram".to_string(), 2), ("openai".to_string(), 1)]
        );
    }

    #[test]
    fn empty_search_returns_all_newest_first() {
        let h = History::open_in_memory().unwrap();
        h.record("first", "deepgram").unwrap();
        h.record("second", "openai").unwrap();
        let all = h.search("").unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].text, "second"); // newest first
    }
}
