//! Disk cache of what the radio has learned about tracks: the analyses, the genres, when a track was played, the
//! settings, the blends there were. Stage 2 of docs/rust-port.md, ported from `cache.py`.
//!
//! It is the very SQLite file of the Python radio (the same tables and columns; it was `~/.cache/yamusic-radio.db`): the tables, the columns and the
//! JSON in them are the same, so a cache made by one program is read by the other and nothing that was learned is
//! lost on the way over. Only metadata is stored, never audio.

use crate::mix::{self, TrackAnalysis, key};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// `~/.cache/endlesswave.db`
pub fn default_cache() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|_| crate::paths::cache_file())
}

/// Rows kept; the ones not used for the longest time go first.
const MAX_ROWS: i64 = 20000;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS tracks (
    track_id TEXT NOT NULL,
    codec TEXT NOT NULL,
    title TEXT NOT NULL,
    analysis TEXT NOT NULL,
    genre TEXT NOT NULL DEFAULT '',
    played_at REAL,
    used_at REAL NOT NULL,
    PRIMARY KEY (track_id, codec)
);
CREATE TABLE IF NOT EXISTS transitions (
    at REAL NOT NULL,
    kind TEXT NOT NULL,
    beat_matched INTEGER NOT NULL,
    key_relation TEXT NOT NULL,
    from_track TEXT NOT NULL,
    to_track TEXT NOT NULL,
    from_key TEXT,
    to_key TEXT,
    effect TEXT
);
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS tracks_played ON tracks (played_at);
CREATE INDEX IF NOT EXISTS tracks_used ON tracks (used_at);
";

/// Transitions counted by a column; what was made before the column was kept is "unknown".
fn count_by(column: &str) -> String {
    format!(
        "SELECT COALESCE({column}, 'unknown'), COUNT(*) FROM transitions \
         GROUP BY COALESCE({column}, 'unknown') ORDER BY COUNT(*) DESC"
    )
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

/// What is stored in the `analysis` column: the analysis and the names of what found its key and its loudness.
/// The fields are the ones of `TrackAnalysis` of the Python radio; one that is not known (a cache of another
/// version) is a reason to analyze the track again.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    bpm: f64,
    head_beat: f64,
    tail_beat: f64,
    start: i64,
    end: i64,
    loudness_db: f64,
    #[serde(default = "minus_one")]
    intro_end: i64,
    #[serde(default = "minus_one")]
    outro_start: i64,
    #[serde(default = "minus_one")]
    key: i32,
    key_by: Option<String>,
    loudness_by: Option<String>,
}

fn minus_one<T: From<i8>>() -> T {
    T::from(-1)
}

/// How one track was blended into the next.
#[derive(Debug, Clone, Default)]
pub struct NewTransition<'a> {
    pub kind: &'a str,
    pub beat_matched: bool,
    pub key_relation: &'a str,
    pub from_track: &'a str,
    pub to_track: &'a str,
    /// Camelot codes, empty when a key was not found.
    pub from_key: &'a str,
    pub to_key: &'a str,
    pub effect: &'a str,
}

/// The transitions ever made, counted (`transition_stats`). Each list has the most common first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionStats {
    pub total: i64,
    pub beat_matched: i64,
    pub by_type: Vec<(String, i64)>,
    pub by_key: Vec<(String, i64)>,
    pub by_quality: key::QualityCounts,
    pub by_effect: Vec<(String, i64)>,
}

/// SQLite store of track analyses and play times.
pub struct TrackCache {
    db: Connection,
    max_rows: i64,
}

impl TrackCache {
    /// Open the cache, make the tables if there are none, and add the columns that a cache made by an older
    /// version lacks.
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        }
        let db = Connection::open(path)?;
        db.execute_batch(SCHEMA)?;
        let columns = |table: &str| -> rusqlite::Result<HashSet<String>> {
            db.prepare(&format!("PRAGMA table_info({table})"))?.query_map([], |row| row.get(1))?.collect()
        };
        let transition_columns = columns("transitions")?;
        for column in ["from_key", "to_key", "effect"] {
            if !transition_columns.contains(column) {
                // Made before the keys (or the effects) of the transitions were kept: those rows have none (NULL),
                // unlike a key that was not found, or a blend without effects.
                db.execute(&format!("ALTER TABLE transitions ADD COLUMN {column} TEXT"), [])?;
            }
        }
        if !columns("tracks")?.contains("genre") {
            // A cache made before the genre was kept.
            db.execute("ALTER TABLE tracks ADD COLUMN genre TEXT NOT NULL DEFAULT ''", [])?;
        }
        Ok(Self { db, max_rows: MAX_ROWS })
    }

    /// Keep this many rows of tracks instead of 20000 (for the tests).
    pub fn with_max_rows(mut self, rows: i64) -> Self {
        self.max_rows = rows;
        self
    }

    /// What `put_setting` kept under the key, `None` if nothing (or something that is not JSON).
    pub fn get_setting(&self, key: &str) -> Option<Value> {
        let text: String =
            self.db.query_row("SELECT value FROM settings WHERE key = ?", [key], |row| row.get(0)).optional().ok()??;
        serde_json::from_str(&text).ok()
    }

    /// Keep a JSON value (the moods picked, ...) across restarts.
    pub fn put_setting(&self, key: &str, value: &Value) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value.to_string()],
        )?;
        Ok(())
    }

    pub fn delete_setting(&self, key: &str) -> rusqlite::Result<()> {
        self.db.execute("DELETE FROM settings WHERE key = ?", [key])?;
        Ok(())
    }

    /// The analysis of a track, if it is cached and was made by what makes it now.
    pub fn get_analysis(&self, track_id: &str, codec: &str) -> Option<TrackAnalysis> {
        self.get_analysis_by(track_id, codec, mix::key_detector(), mix::loudness_meter())
    }

    /// The analysis of a track, if it was made by the given key detector and loudness meter. Not the current
    /// ones: the key was not analyzed, or by something worse, or the loudness was measured another way; the track
    /// is analyzed again.
    pub fn get_analysis_by(&self, track_id: &str, codec: &str, key_detector: &str, loudness_meter: &str) -> Option<TrackAnalysis> {
        let text: String = self
            .db
            .query_row("SELECT analysis FROM tracks WHERE track_id = ? AND codec = ?", [track_id, codec], |row| row.get(0))
            .optional()
            .ok()??;
        // Written by another version of the program: analyze again.
        let stored: Stored = serde_json::from_str(&text).ok()?;
        if stored.key_by.as_deref() != Some(key_detector) || stored.loudness_by.as_deref() != Some(loudness_meter) {
            return None;
        }
        if stored.key < 0 && key_detector == "essentia" {
            // No key was found, perhaps by an older detector: try again.
            return None;
        }
        Some(TrackAnalysis {
            bpm: stored.bpm,
            head_beat: stored.head_beat,
            tail_beat: stored.tail_beat,
            start: stored.start,
            end: stored.end,
            loudness_db: stored.loudness_db,
            intro_end: stored.intro_end,
            outro_start: stored.outro_start,
            key: stored.key,
        })
    }

    pub fn put_analysis(&self, track_id: &str, codec: &str, title: &str, analysis: &TrackAnalysis, genre: &str) -> rusqlite::Result<()> {
        self.put_analysis_by(track_id, codec, title, analysis, genre, mix::key_detector(), mix::loudness_meter())
    }

    /// Keep the analysis of a track, made by the given key detector and loudness meter.
    #[allow(clippy::too_many_arguments)]
    pub fn put_analysis_by(
        &self,
        track_id: &str,
        codec: &str,
        title: &str,
        analysis: &TrackAnalysis,
        genre: &str,
        key_detector: &str,
        loudness_meter: &str,
    ) -> rusqlite::Result<()> {
        let mut stored = serde_json::to_value(analysis).expect("an analysis is JSON");
        stored["key_by"] = key_detector.into();
        stored["loudness_by"] = loudness_meter.into();
        self.db.execute(
            "INSERT INTO tracks (track_id, codec, title, analysis, genre, used_at) VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT (track_id, codec) DO UPDATE \
             SET title = excluded.title, analysis = excluded.analysis, genre = excluded.genre, used_at = excluded.used_at",
            params![track_id, codec, title, stored.to_string(), genre, now()],
        )?;
        self.prune()
    }

    /// Keep the genre of a track that is already in the cache.
    pub fn set_genre(&self, track_id: &str, codec: &str, genre: &str) -> rusqlite::Result<()> {
        self.db.execute("UPDATE tracks SET genre = ? WHERE track_id = ? AND codec = ?", [genre, track_id, codec])?;
        Ok(())
    }

    /// The genre kept for a track, `None` if it isn't cached.
    pub fn get_genre(&self, track_id: &str, codec: &str) -> Option<String> {
        self.db
            .query_row("SELECT genre FROM tracks WHERE track_id = ? AND codec = ?", [track_id, codec], |row| row.get(0))
            .optional()
            .ok()?
    }

    /// Remember how one track was blended into the next.
    pub fn add_transition(&self, t: &NewTransition) -> rusqlite::Result<()> {
        self.db.execute(
            "INSERT INTO transitions (at, kind, beat_matched, key_relation, from_track, to_track, from_key, to_key, effect) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![now(), t.kind, i32::from(t.beat_matched), t.key_relation, t.from_track, t.to_track, t.from_key, t.to_key, t.effect],
        )?;
        Ok(())
    }

    /// Every key change there was, the most common first.
    pub fn key_pairs(&self) -> rusqlite::Result<Vec<(String, String, i64)>> {
        self.db
            .prepare(
                "SELECT from_key, to_key, COUNT(*) AS n FROM transitions \
                 WHERE from_key IS NOT NULL AND to_key IS NOT NULL \
                 GROUP BY from_key, to_key ORDER BY n DESC, from_key, to_key",
            )?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect()
    }

    /// Count the transitions ever made by kind and by key change.
    pub fn transition_stats(&self) -> rusqlite::Result<TransitionStats> {
        let count = |column: &str| -> rusqlite::Result<Vec<(String, i64)>> {
            self.db.prepare(&count_by(column))?.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?.collect()
        };
        let (total, beat_matched) = self.db.query_row(
            "SELECT COUNT(*), COALESCE(SUM(beat_matched), 0) FROM transitions",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let by_key = count("key_relation")?;
        Ok(TransitionStats {
            total,
            beat_matched,
            by_type: count("kind")?,
            by_quality: key::quality_counts(by_key.iter().map(|(name, n)| (name.as_str(), *n))),
            by_key,
            by_effect: count("effect")?,
        })
    }

    /// The genres of the cached tracks, the most common first.
    pub fn known_genres(&self) -> rusqlite::Result<Vec<(String, i64)>> {
        self.db
            .prepare(
                "SELECT genre, COUNT(DISTINCT track_id) AS n FROM tracks \
                 WHERE genre != '' GROUP BY genre ORDER BY n DESC, genre",
            )?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect()
    }

    pub fn mark_played(&self, track_id: &str, codec: &str) -> rusqlite::Result<()> {
        let now = now();
        self.db.execute(
            "UPDATE tracks SET played_at = ?, used_at = ? WHERE track_id = ? AND codec = ?",
            params![now, now, track_id, codec],
        )?;
        Ok(())
    }

    /// The ids of the `limit` tracks played most recently.
    pub fn recently_played(&self, limit: i64) -> rusqlite::Result<HashSet<String>> {
        self.db
            .prepare(
                "SELECT DISTINCT track_id FROM tracks WHERE played_at IS NOT NULL \
                 ORDER BY played_at DESC LIMIT ?",
            )?
            .query_map([limit], |row| row.get(0))?
            .collect()
    }

    fn prune(&self) -> rusqlite::Result<()> {
        self.db.execute(
            "DELETE FROM tracks WHERE rowid IN (SELECT rowid FROM tracks ORDER BY used_at DESC LIMIT -1 OFFSET ?)",
            [self.max_rows],
        )?;
        Ok(())
    }
}
