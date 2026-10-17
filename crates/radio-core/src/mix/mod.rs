//! The audio core: analysis of a track, planning and making a blend, the choice of the next track.
//! Ported from `yamusic_radio/dj_mix.py`; the numbers are the same, so the tests can compare them.

use serde::Deserialize;

pub const SAMPLE_RATE: u32 = 44100;
pub const CHANNELS: usize = 2;

/// Stereo samples, interleaved: `[left, right, left, right, ...]`.
pub type Pcm = Vec<f32>;

/// What the analysis finds in a track (`TrackAnalysis` of `dj_mix.py`).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TrackAnalysis {
    /// 0 when the tempo could not be told.
    pub bpm: f64,
    /// Samples: a beat near the start of the track.
    pub head_beat: f64,
    /// Samples: a beat near the end of the track.
    pub tail_beat: f64,
    /// The first audible sample.
    pub start: i64,
    /// One past the last audible sample.
    pub end: i64,
    /// LUFS (EBU R128).
    pub loudness_db: f64,
    /// Where the quiet intro ends, -1 when it could not be told.
    pub intro_end: i64,
    /// Where the quiet outro begins, -1 when it could not be told.
    pub outro_start: i64,
    /// 0-11 a major key (C, C#, ... B), 12-23 the minor key with tonic 0-11, -1 when it could not be told.
    pub key: i32,
}

/// Analyze a track. Stage 1 of docs/rust-port.md (1.3 tempo, 1.4 loudness, 1.5 key).
pub fn analyze(_pcm: &[f32]) -> TrackAnalysis {
    todo!("stage 1 of docs/rust-port.md")
}
