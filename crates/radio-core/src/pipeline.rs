//! The mix as a series of pieces: the decks (decoded tracks), where a blend goes, and how the pieces are cut. Tasks 3.2.1
//! onwards of docs/rust-port-radio.md, ported from `dj.py`: `Deck`, `BlendPlan`, `_pieces`, `_plan`, `_early_plan`,
//! `_beat_period`, `_followed_until`.

use crate::{
    backend::Track,
    mix::{CHANNELS, SAMPLE_RATE, TrackAnalysis, glide::NEGLIGIBLE_TEMPO_SHIFT, plan},
};
use std::sync::Arc;

/// The mix is produced in pieces of this length.
pub const CHUNK_SECONDS: f64 = 0.25;
/// The next track is chosen and decoded this long before the blend.
pub const LOOKAHEAD_SECONDS: f64 = 40.0;
/// A beat grid of a blend needs at least this many points to give a period.
const MIN_BEATS_IN_GRID: usize = 2;
/// A blend asked for at once (the skip button) is let go with less room than this.
const MIN_EARLY_BLEND_SECONDS: f64 = plan::MIN_EARLY_BLEND_SECONDS;

/// A piece of the mix, and the title of what is heard in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    /// Stereo, interleaved.
    pub pcm: Vec<f32>,
    pub title: Arc<str>,
}

/// A decoded track as it will be played.
#[derive(Debug, Clone)]
pub struct Deck {
    pub title: Arc<str>,
    /// Stereo, interleaved, 44.1 kHz.
    pub pcm: Arc<[f32]>,
    pub analysis: TrackAnalysis,
    pub beat_matched: bool,
    pub track: Option<Track>,
    pub mood: String,
    /// The ratio of the tempo of the track before it to its own tempo (a beat-matched blend glides through it, see
    /// `glide::tempo_glide`).
    pub speed: f64,
    /// `Radio::generation` when the deck was chosen.
    pub generation: u64,
}

impl Deck {
    /// How many frames of sound the deck holds.
    pub fn frames(&self) -> i64 {
        (self.pcm.len() / CHANNELS) as i64
    }
}

/// Where a blend takes its pieces from (samples), and at what pace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlendPlan {
    /// In the outgoing track.
    pub start: i64,
    /// In the incoming track.
    pub following_start: i64,
    /// Of the outgoing piece.
    pub length: i64,
    /// Incoming samples per outgoing sample.
    pub speed: f64,
}

/// Cut a track into pieces of a quarter of a second, the last one shorter. The pieces are made one at a time.
pub fn pieces<'a>(pcm: &'a [f32], title: &'a Arc<str>) -> impl Iterator<Item = Piece> + 'a {
    let size = (CHUNK_SECONDS * f64::from(SAMPLE_RATE)).round_ties_even() as usize * CHANNELS;
    pcm.chunks(size).map(move |chunk| Piece { pcm: chunk.to_vec(), title: Arc::clone(title) })
}

/// Samples of a stretch of the following deck from `start` at `speed`: the Python `int(frames / speed)`.
fn playable(following: &Deck, following_start: i64, speed: f64) -> i64 {
    ((following.frames() - following_start) as f64 / speed) as i64
}

/// Plan the blend of the following deck into the current one. A deck that has no room for a blend on the beat is not
/// beat-matched any more (`following.beat_matched` says so).
pub fn plan_blend(current: &Deck, following: &mut Deck, position: i64) -> BlendPlan {
    let matched = if following.beat_matched {
        plan::plan_matched(&current.analysis, &following.analysis, following.speed)
    } else {
        None
    };
    // Without room for a blend on the beat the tracks are not matched.
    following.beat_matched = matched.is_some();
    let planned = matched.unwrap_or_else(|| plan::plan_blend(&current.analysis, &following.analysis, false, 1.0));
    let speed = if following.beat_matched { following.speed } else { 1.0 };
    let start = planned.start.max(position);
    let length = 0.max(
        planned
            .length
            .min(current.analysis.end - start)
            // The decoded track may be a few samples shorter than its analysis says (rounding).
            .min(current.frames() - start)
            .min(playable(following, planned.following_start, speed)),
    );
    BlendPlan { start, following_start: planned.following_start, length, speed }
}

/// Plan the blend asked for by the skip button, if it can be made now. It needs some room left in both tracks. (The
/// caller has seen that the following deck is ready and not stale.)
pub fn plan_early(current: &Deck, following: &Deck, position: i64) -> Option<BlendPlan> {
    let speed = if following.beat_matched { following.speed } else { 1.0 };
    let planned = plan::plan_early(&current.analysis, &following.analysis, position, following.beat_matched, speed)?;
    let length = planned.length.min(current.frames() - planned.start).min(playable(following, planned.following_start, speed));
    if (length as f64) < MIN_EARLY_BLEND_SECONDS * f64::from(SAMPLE_RATE) {
        return None;
    }
    Some(BlendPlan { start: planned.start, following_start: planned.following_start, length, speed })
}

/// The samples per beat of the blend, 0 if the beats don't match.
pub fn beat_period(current: &Deck, following: &Deck, beat_grid: Option<&[f64]>) -> f64 {
    if !following.beat_matched {
        return 0.0;
    }
    match beat_grid {
        Some(grid) if grid.len() >= MIN_BEATS_IN_GRID => (grid[grid.len() - 1] - grid[0]) / (grid.len() - 1) as f64,
        _ => current.analysis.beat_period(),
    }
}

/// Where in the following deck its own tempo goes on after a blend. A blend that glided takes `speed` times as much of
/// the following track as of the outgoing one, however long the glide made the blend.
pub fn followed_until(plan: &BlendPlan, following: &Deck) -> i64 {
    let glided = following.beat_matched && (plan.speed - 1.0).abs() >= NEGLIGIBLE_TEMPO_SHIFT;
    plan.following_start + (plan.length as f64 * if glided { plan.speed } else { 1.0 }).round_ties_even() as i64
}
