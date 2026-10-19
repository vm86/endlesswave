//! What the radio keeps about tracks, and the errors it answers with. Task 3.1.1 of docs/rust-port-radio.md, ported
//! from `dj.py`: `Candidate`, `Played`, `TrackInfo`, `SkipRefusedError`, `QueueChangeRefusedError`,
//! `UnknownQueueTrackError`.

use crate::{
    backend::Track,
    mix::{SAMPLE_RATE, TrackAnalysis, key},
    pipeline::round_to,
};
use serde::Serialize;
use std::{collections::HashSet, path::PathBuf};

/// The API lists at most this many tracks, before and after the current one.
pub const MAX_LISTED: usize = 15;

/// A downloaded and analyzed track waiting for its turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub title: String,
    pub path: PathBuf,
    pub analysis: TrackAnalysis,
    pub artists: HashSet<String>,
    pub track: Option<Track>,
    /// The station mood the track was fetched with.
    pub mood: String,
    pub genre: String,
}

/// What is kept of a track once it has been played, to repeat it.
#[derive(Debug, Clone)]
pub struct Played {
    pub track: Track,
    pub title: String,
    pub analysis: TrackAnalysis,
    pub artists: HashSet<String>,
}

/// A track as the API shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrackInfo {
    pub track_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub bpm: f64,
    pub loudness_db: f64,
    pub duration_seconds: f64,
    pub mood: String,
    /// Camelot Wheel code and the name of the key; "" when it isn't known.
    pub camelot: String,
    pub key: String,
    /// False for the tracks of a forecast: the radio may still change them.
    pub decided: bool,
    /// The cover's URL, "%%" for the size; "" when there is none.
    pub cover: String,
    /// Put where it is by hand: the radio doesn't move it.
    pub pinned: bool,
    /// The id of the backend the track is from ("yandex", "vk", "local"); "" when the radio has just one backend.
    pub source: String,
}

impl TrackInfo {
    pub fn of(
        title: &str,
        analysis: &TrackAnalysis,
        track: Option<&Track>,
        mood: &str,
        decided: bool,
        pinned: bool,
    ) -> Self {
        Self {
            track_id: track.map_or_else(String::new, |t| t.track_id.clone()),
            title: title.to_owned(),
            artists: track.map_or_else(Vec::new, |t| t.artists.clone()),
            bpm: round_to(analysis.bpm, 1),
            loudness_db: round_to(analysis.loudness_db, 1),
            duration_seconds: round_to(
                (analysis.end - analysis.start) as f64 / f64::from(SAMPLE_RATE),
                1,
            ),
            mood: mood.to_owned(),
            camelot: key::camelot_code(analysis.key),
            key: key::key_name(analysis.key),
            decided,
            cover: track.map_or_else(String::new, |t| t.cover.clone()),
            pinned,
            source: track.map_or_else(String::new, |t| t.source.clone()),
        }
    }
}

/// What the radio refuses to do, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RadioError {
    /// The next track can't be asked for now.
    SkipRefused(String),
    /// The queue can't be changed that way now (a blend is under way).
    QueueChangeRefused(String),
    /// The track is not in the queue.
    UnknownQueueTrack(String),
    /// A value that is not one of the choices (`ValueError` of Python).
    Invalid(String),
    /// The backend could not do it.
    Backend(String),
}

impl std::fmt::Display for RadioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SkipRefused(why)
            | Self::QueueChangeRefused(why)
            | Self::Invalid(why)
            | Self::Backend(why) => f.write_str(why),
            Self::UnknownQueueTrack(id) => f.write_str(id),
        }
    }
}

impl std::error::Error for RadioError {}

/// Counts in the order of first sight, like Python's `Counter`: `most_common` is by count, and of equal counts the one
/// that came first is first.
#[derive(Debug, Clone, Default)]
pub struct Counter {
    items: Vec<(String, i64)>,
}

impl Counter {
    pub fn add(&mut self, key: &str) {
        match self.items.iter_mut().find(|(k, _)| k == key) {
            Some((_, n)) => *n += 1,
            None => self.items.push((key.to_owned(), 1)),
        }
    }

    pub fn total(&self) -> i64 {
        self.items.iter().map(|(_, n)| n).sum()
    }

    pub fn most_common(&self) -> Vec<(String, i64)> {
        let mut items = self.items.clone();
        items.sort_by_key(|item| std::cmp::Reverse(item.1)); // stable
        items
    }
}
