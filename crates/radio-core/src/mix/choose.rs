//! The choice of the next track: the energy arc, the moods, the cost of a transition and the plan of the
//! next few tracks as a whole. Task 1.10 of docs/rust-port.md, ported from `dj_mix.py`: `energy`,
//! `arc_target`, `mood_for`, `mood_of`, `nearest_mood`, `mood_cost`, `transition_cost`, `step_cost`,
//! `_best_path`, `choose_next`.

use super::{TrackAnalysis, key, plan};
use std::{collections::HashSet, f64::consts::PI};

// Costs of a transition when choosing the next track. They are in the same unit, "how much worse than a
// perfect blend".
const MATCHED_TEMPO_COST: f64 = 10.0; // per octave of tempo change
const UNMATCHED_COST: f64 = 4.0; // beats can't be lined up
const UNMATCHED_TEMPO_COST: f64 = 5.0; // per octave of tempo gap, at most one octave
const UNKNOWN_TEMPO_COST: f64 = 2.5; // on top of UNMATCHED_COST
const SWEEP_COST: f64 = 1.0; // a filter sweep instead of a bass swap
const PLAIN_LENGTH_COST: f64 = 0.5; // no intro/outro to size the blend by
const LOUDNESS_COST: f64 = 0.1; // per dB
const REPEAT_ARTIST_COST: f64 = 6.0;
const ENERGY_ARC_COST: f64 = 8.0; // per unit of distance from the target energy

/// The queue is planned this many tracks ahead, trying the best few candidates at each step; later steps
/// count for less.
const LOOKAHEAD_WEIGHTS: [f64; 3] = [1.0, 0.6, 0.4];
const LOOKAHEAD_BEAM: usize = 8;

/// The station mood wanted at each point of the arc, calm to lively.
const MOODS: [&str; 3] = ["calm", "fun", "active"];
/// Every mood the station can be set to (quietest first); "all" is no choice.
pub const ALL_MOODS: [&str; 4] = ["sad", "calm", "fun", "active"];
const MOOD_SAD_BELOW: f64 = 0.2; // and in a minor key
const MOOD_CALM_BELOW: f64 = 0.3;
const MOOD_FUN_BELOW: f64 = 0.5;
const MOOD_COST: f64 = 2.0;
/// A track by a recently played artist is penalized.
const REPEAT_WINDOW: usize = 4;
/// A set builds up and winds down over this many tracks.
const ARC_TRACKS: f64 = 10.0;
const ENERGY_BPM_RANGE: (f64, f64) = (90.0, 150.0);
/// In LUFS.
const ENERGY_LOUDNESS_RANGE_DB: (f64, f64) = (-16.0, -4.0);

/// A track waiting in the pool, as far as choosing it is concerned.
#[derive(Debug, Clone)]
pub struct Entry {
    pub analysis: TrackAnalysis,
    pub artists: HashSet<String>,
    /// The station mood the track was fetched with, "" when not known.
    pub mood: String,
}

/// How intense a track is, 0..1: faster and louder is more.
pub fn energy(analysis: &TrackAnalysis) -> f64 {
    let scaled = |value: f64, (low, high): (f64, f64)| ((value - low) / (high - low)).clamp(0.0, 1.0);
    let loudness = scaled(analysis.loudness_db, ENERGY_LOUDNESS_RANGE_DB);
    if analysis.bpm == 0.0 {
        return loudness;
    }
    (scaled(analysis.bpm, ENERGY_BPM_RANGE) + loudness) / 2.0
}

/// The energy wanted at the given place in the set: it starts calm, builds to a peak half way through the arc
/// and winds down again, over and over.
pub fn arc_target(position: i64) -> f64 {
    0.5 - 0.35 * (2.0 * PI * position as f64 / ARC_TRACKS).cos()
}

/// The station mood that goes with the place in the set.
pub fn mood_for(position: i64) -> &'static str {
    let target = arc_target(position);
    if target < MOOD_CALM_BELOW {
        "calm"
    } else if target < MOOD_FUN_BELOW {
        "fun"
    } else {
        "active"
    }
}

/// The mood a track sounds like, by its energy and its key; very quiet music in a minor key is sad.
pub fn mood_of(analysis: &TrackAnalysis) -> &'static str {
    let level = energy(analysis);
    if level < MOOD_SAD_BELOW && analysis.key >= 12 {
        "sad"
    } else if level < MOOD_CALM_BELOW {
        "calm"
    } else if level < MOOD_FUN_BELOW {
        "fun"
    } else {
        "active"
    }
}

/// The mood among the allowed ones that is closest to `wanted`; everything is allowed when the list is empty.
pub fn nearest_mood<'a>(wanted: &'a str, allowed: &'a [&'a str]) -> &'a str {
    if allowed.is_empty() || allowed.contains(&wanted) {
        return wanted;
    }
    let Some(place) = ALL_MOODS.iter().position(|&m| m == wanted) else { return allowed[0] };
    let distance = |mood: &str| ALL_MOODS.iter().position(|&m| m == mood).map_or(usize::MAX, |p| p.abs_diff(place));
    // The first of the closest, as min() says.
    allowed.iter().copied().min_by_key(|m| distance(m)).expect("the list is not empty")
}

/// The cost of a track picked for another mood than wanted.
pub fn mood_cost(wanted: &str, got: &str) -> f64 {
    let (Some(wanted), Some(got)) = (MOODS.iter().position(|&m| m == wanted), MOODS.iter().position(|&m| m == got))
    else {
        return 0.0;
    };
    MOOD_COST * wanted.abs_diff(got) as f64
}

/// Judge the transition the way the radio would actually make it.
pub fn transition_cost(current: &TrackAnalysis, following: &TrackAnalysis) -> f64 {
    let speed = plan::transition_speed(current.bpm, following);
    let mut cost;
    if speed != 0.0 {
        cost = MATCHED_TEMPO_COST * speed.log2().abs();
        if plan::choose_transition(current, following, true, 0.0) == plan::Transition::FilterSweep {
            cost += SWEEP_COST;
        }
    } else if current.bpm != 0.0 && following.bpm != 0.0 {
        let gap = plan::tempo_ratio(current.bpm, following.bpm).log2().abs();
        cost = UNMATCHED_COST + UNMATCHED_TEMPO_COST * gap.min(1.0);
    } else {
        cost = UNMATCHED_COST + UNKNOWN_TEMPO_COST;
    }

    if plan::section_length(current, following).is_none() {
        cost += PLAIN_LENGTH_COST;
    }
    cost += LOUDNESS_COST * (current.loudness_db - following.loudness_db).abs();
    cost += key::key_cost(current.key, following.key);
    cost
}

/// Cost of playing the entry after `current`.
pub fn step_cost(current: Option<&TrackAnalysis>, entry: &Entry, history: &[HashSet<String>], position: i64) -> f64 {
    let mut cost = current.map_or(0.0, |current| transition_cost(current, &entry.analysis));
    let recent = &history[history.len().saturating_sub(REPEAT_WINDOW)..];
    if recent.iter().any(|artists| !entry.artists.is_disjoint(artists)) {
        cost += REPEAT_ARTIST_COST;
    }
    cost += ENERGY_ARC_COST * (energy(&entry.analysis) - arc_target(position)).abs();
    cost += mood_cost(mood_for(position), &entry.mood);
    cost
}

/// The cost of the best queue from here and its first pick.
fn best_path(
    current: Option<&TrackAnalysis>,
    pool: &[&Entry],
    history: &[HashSet<String>],
    position: i64,
    depth: usize,
) -> (f64, usize) {
    let mut steps: Vec<(f64, usize)> =
        pool.iter().enumerate().map(|(index, entry)| (step_cost(current, entry, history, position), index)).collect();
    // A stable sort: of equal costs the first stays first.
    steps.sort_by(|a, b| a.0.total_cmp(&b.0));

    let (mut best_cost, mut best_index) = (f64::INFINITY, 0);
    for &(cost, index) in steps.iter().take(LOOKAHEAD_BEAM) {
        let mut total = LOOKAHEAD_WEIGHTS[LOOKAHEAD_WEIGHTS.len() - depth] * cost;
        if depth > 1 && pool.len() > 1 {
            let rest: Vec<&Entry> = pool.iter().enumerate().filter(|&(i, _)| i != index).map(|(_, e)| *e).collect();
            let mut later = history.to_vec();
            later.push(pool[index].artists.clone());
            total += best_path(Some(&pool[index].analysis), &rest, &later, position + 1, depth - 1).0;
        }
        if total < best_cost {
            (best_cost, best_index) = (total, index);
        }
    }
    (best_cost, best_index)
}

/// The index in the pool of the track to play after `current`. It plans the next few tracks as a whole: a track
/// that blends perfectly but leaves nothing that fits after it is not a good pick.
///
/// `history` has the artists of the tracks played, the oldest first. The pool must not be empty.
pub fn choose_next(current: Option<&TrackAnalysis>, pool: &[Entry], history: &[HashSet<String>], position: i64) -> usize {
    let depth = LOOKAHEAD_WEIGHTS.len().min(pool.len());
    let pool: Vec<&Entry> = pool.iter().collect();
    best_path(current, &pool, history, position, depth).1
}

/// As `choose_next`, with the cost of the best queue too (for the tests).
pub fn best_queue_cost(current: Option<&TrackAnalysis>, pool: &[Entry], history: &[HashSet<String>], position: i64) -> f64 {
    let depth = LOOKAHEAD_WEIGHTS.len().min(pool.len());
    let pool: Vec<&Entry> = pool.iter().collect();
    best_path(current, &pool, history, position, depth).0
}
