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

// Where a blend goes, task 1.6 of docs/rust-port.md. Ported from `dj_mix.py`: `_blend_beats`, `_first_beat`,
// `plan_matched`, `plan_early`, `plan_blend`.

/// Blend length when the structure of the tracks is unknown.
const BEATS_PER_BLEND: i64 = 16;
const PLAIN_BLEND_SECONDS: f64 = 8.0;
/// Blends follow the shorter of the outgoing outro and the incoming intro, within these limits (beats are
/// counted in steps of two bars).
const MIN_BLEND_BEATS: i64 = 8;
const MAX_BLEND_BEATS: i64 = 32;
const BLEND_BEATS_STEP: i64 = 8;
const MIN_PLAIN_BLEND_SECONDS: f64 = 4.0;
const MAX_PLAIN_BLEND_SECONDS: f64 = 16.0;
/// A matched blend must leave at least this much of the outgoing track.
const MIN_SOLO_SECONDS: f64 = 10.0;
/// A blend asked for at once (the skip button) starts no sooner than this after the request, on the first bar
/// line of the outgoing track after it.
const EARLY_LEAD_SECONDS: f64 = 2.0;
/// Less room than this in either track and the request is let go.
pub const MIN_EARLY_BLEND_SECONDS: f64 = 4.0;
/// A bar is this many beats.
const BEATS_PER_BAR: f64 = 4.0;

/// Python's `round()` of a float: halves go to the even number.
fn round(value: f64) -> i64 {
    value.round_ties_even() as i64
}

/// Where the blend starts in each track and how long it lasts, in samples of the outgoing track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub start: i64,
    pub following_start: i64,
    pub length: i64,
}

impl TrackAnalysis {
    /// Samples per beat. The tempo must be known.
    pub fn beat_period(&self) -> f64 {
        f64::from(SAMPLE_RATE) * 60.0 / self.bpm
    }
}

fn blend_beats(section: Option<i64>, period: f64) -> i64 {
    let Some(section) = section else { return BEATS_PER_BLEND };
    let beats = round(section as f64 / period / BLEND_BEATS_STEP as f64) * BLEND_BEATS_STEP;
    beats.clamp(MIN_BLEND_BEATS, MAX_BLEND_BEATS)
}

/// The first beat of the track after its silence (samples).
fn first_beat(track: &TrackAnalysis) -> i64 {
    let j = ((track.start as f64 - track.head_beat) / track.beat_period()).ceil();
    round(track.head_beat + j * track.beat_period()).max(0)
}

/// Plan a blend on the beat, `None` if there is no room for one. `following` is at its own tempo: the blend
/// glides to it, so it takes `speed` times as much of the following track as of the outgoing one.
pub fn plan_matched(current: &TrackAnalysis, following: &TrackAnalysis, speed: f64) -> Option<Plan> {
    let period = current.beat_period();
    let solo = round(MIN_SOLO_SECONDS * f64::from(SAMPLE_RATE));
    let following_start = first_beat(following);

    let section = section_length(current, following);
    let mut beats = blend_beats(section, period);
    let current_start = loop {
        let length = round(beats as f64 * period);
        // The blend ends with the outgoing track, on a beat of its grid.
        let k = ((current.end - length) as f64 - current.tail_beat) / period;
        let current_start = round(current.tail_beat + k.floor() * period);
        if section.is_none() || beats <= MIN_BLEND_BEATS {
            break current_start;
        }
        // Rounding up to whole bars mustn't reach into the cores.
        let into_outgoing_core = (current_start as f64) < current.outro_start as f64 - period / 2.0;
        let into_incoming_core = following_start as f64 + length as f64 * speed > following.intro_end as f64 + period / 2.0;
        if !(into_outgoing_core || into_incoming_core) {
            break current_start;
        }
        beats -= BLEND_BEATS_STEP;
    };

    if current_start < current.start + solo {
        return None;
    }
    let length = (current.end - current_start).min(((following.end - following_start) as f64 / speed) as i64);
    Some(Plan { start: current_start, following_start, length })
}

/// Plan a blend that is asked for while the outgoing track is at `position`. It starts on a bar line of the
/// outgoing track, the first one at least `EARLY_LEAD_SECONDS` ahead (the beat grid is taken from the end of the
/// track that is nearer), lasts as long as any blend without sections and comes in on the first beat of the
/// following track. Without a shared grid it starts as soon as the lead allows. `None` if the outgoing track
/// has no room left for it.
pub fn plan_early(
    current: &TrackAnalysis,
    following: &TrackAnalysis,
    position: i64,
    beat_matched: bool,
    speed: f64,
) -> Option<Plan> {
    let earliest = position + round(EARLY_LEAD_SECONDS * f64::from(SAMPLE_RATE));
    let (start, following_start, length);
    if beat_matched && current.bpm != 0.0 && following.bpm != 0.0 {
        let period = current.beat_period();
        let anchor = if (position as f64 - current.head_beat) < current.tail_beat - position as f64 {
            current.head_beat
        } else {
            current.tail_beat
        };
        let bar = BEATS_PER_BAR * period;
        start = round(anchor + ((earliest as f64 - anchor) / bar).ceil() * bar);
        following_start = first_beat(following);
        length = round(BEATS_PER_BLEND as f64 * period);
    } else {
        start = earliest;
        following_start = following.start;
        length = round(PLAIN_BLEND_SECONDS * f64::from(SAMPLE_RATE));
    }
    let length = length.min(current.end - start).min(((following.end - following_start) as f64 / speed) as i64);
    if (length as f64) < MIN_EARLY_BLEND_SECONDS * f64::from(SAMPLE_RATE) {
        return None;
    }
    Some(Plan { start, following_start, length })
}

/// Where the blend starts in each track and how long it lasts.
pub fn plan_blend(current: &TrackAnalysis, following: &TrackAnalysis, beat_matched: bool, speed: f64) -> Plan {
    if beat_matched && current.bpm != 0.0 && following.bpm != 0.0 {
        if let Some(planned) = plan_matched(current, following, speed) {
            return planned;
        }
    }

    let section = section_length(current, following);
    let seconds = section.map_or(PLAIN_BLEND_SECONDS, |section| {
        (section as f64 / f64::from(SAMPLE_RATE)).max(MIN_PLAIN_BLEND_SECONDS).min(MAX_PLAIN_BLEND_SECONDS)
    });
    let length = round(seconds * f64::from(SAMPLE_RATE))
        .min((current.end - current.start).div_euclid(2))
        .min((following.end - following.start).div_euclid(2));
    Plan { start: current.end - length, following_start: following.start, length }
}
