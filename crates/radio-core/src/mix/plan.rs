//! What a pair of tracks needs for a blend, as far as choosing the next track goes (the part of task 1.6 of
//! docs/rust-port.md that `transition_cost` uses): the speed, the section, the kind of blend.
//! Ported from `dj_mix.py`: `tempo_ratio`, `transition_speed`, `section_length`, `choose_transition`.

use super::{SAMPLE_RATE, TrackAnalysis, key};

/// Largest tempo change a blend glides through to match the previous track.
const MAX_TEMPO_SHIFT: f64 = 0.15;
/// Below this the tempos are taken for equal and nothing is stretched.
const NEGLIGIBLE_TEMPO_SHIFT: f64 = 0.002;
/// Intros and outros shorter than this don't count as a section.
const MIN_SECTION_SECONDS: f64 = 4.0;
/// Tracks that overlap this long get in each other's way with their mids.
const EQ_BLEND_MIN_SECONDS: f64 = 10.0;
/// Loudness gap (dB) from which a beat-matched pair is swept, not swapped.
const SWEEP_LOUDNESS_GAP_DB: f64 = 3.0;

/// Playback speed bringing `bpm` to `target_bpm` (half or double time allowed).
pub fn tempo_ratio(target_bpm: f64, bpm: f64) -> f64 {
    [0.5, 1.0, 2.0]
        .map(|factor| target_bpm / (bpm * factor))
        .into_iter()
        .min_by(|a, b| a.log2().abs().total_cmp(&b.log2().abs()))
        .expect("there are three")
}

/// Speed to play the following track at, 0 when it can't be matched.
pub fn transition_speed(current_bpm: f64, following: &TrackAnalysis) -> f64 {
    if current_bpm == 0.0 || following.bpm == 0.0 {
        return 0.0;
    }
    let speed = tempo_ratio(current_bpm, following.bpm);
    if (speed - 1.0).abs() > MAX_TEMPO_SHIFT {
        return 0.0;
    }
    if (speed - 1.0).abs() < NEGLIGIBLE_TEMPO_SHIFT { 1.0 } else { speed }
}

/// The shorter of the outgoing outro and the incoming intro (samples): how Mixxx's AutoDJ sizes a transition.
/// `None` when one of the sections is unknown or too short to speak of.
pub fn section_length(current: &TrackAnalysis, following: &TrackAnalysis) -> Option<i64> {
    if current.outro_start < 0 || following.intro_end < 0 {
        return None;
    }
    let outro = current.end - current.outro_start;
    let intro = following.intro_end - following.start;
    let shortest = outro.min(intro);
    (shortest as f64 >= MIN_SECTION_SECONDS * f64::from(SAMPLE_RATE)).then_some(shortest)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Crossfade,
    BassSwap,
    EqBlend,
    FilterSweep,
}

impl Transition {
    /// The name the API and the statistics give it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Crossfade => "crossfade",
            Self::BassSwap => "bass swap",
            Self::EqBlend => "eq blend",
            Self::FilterSweep => "filter sweep",
        }
    }
}

/// Pick the kind of blend that suits the pair of tracks. Two tracks on the same beat grid share the highs
/// while the bass is handed over; when they overlap for long, or their keys clash, the mids are swapped too;
/// without a shared grid (or with a big jump in loudness) one track is filtered away while the other is
/// filtered in.
pub fn choose_transition(
    current: &TrackAnalysis,
    following: &TrackAnalysis,
    beat_matched: bool,
    blend_seconds: f64,
) -> Transition {
    let loudness_gap = (current.loudness_db - following.loudness_db).abs();
    if !beat_matched || loudness_gap >= SWEEP_LOUDNESS_GAP_DB {
        return Transition::FilterSweep;
    }
    let relation = key::key_relation(current.key, following.key);
    if blend_seconds >= EQ_BLEND_MIN_SECONDS || key::key_quality(relation) == key::KeyQuality::Poor {
        Transition::EqBlend
    } else {
        Transition::BassSwap
    }
}
