//! Keys on the Camelot Wheel and the cost of a change of key, task 1.10 of docs/rust-port.md.
//! Ported from `dj_mix.py`: `camelot_code`, `camelot_key`, `key_name`, `key_relation`, `key_quality`,
//! `quality_counts`, `key_cost`.
//!
//! A key is numbered as in `TrackAnalysis::key`: 0-11 a major key (C, C#, ... B), 12-23 the minor key with
//! tonic 0-11, -1 when it could not be told.

const PITCH_CLASSES: i32 = 12;
const KEYS: i32 = 2 * PITCH_CLASSES;
const NOTES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Harmonic mixing on the Camelot Wheel, see `key_cost`.
const KEY_COST_SCALE: f64 = 0.7;
const KEY_NEIGHBOUR_COST: f64 = 0.3;
const KEY_BOOST_COST: f64 = 1.2;
const KEY_SEMITONE_COST: f64 = 1.5;
const KEY_DOWN_COST: f64 = 1.8;
const KEY_DIAGONAL_COST: f64 = 2.5;
const KEY_CLASH_COST: f64 = 4.0;

/// The place of a key on the wheel: the number 1-12 and whether it is a minor key (letter A).
fn camelot(key: i32) -> Option<(i32, bool)> {
    if !(0..KEYS).contains(&key) {
        return None;
    }
    let minor = key >= PITCH_CLASSES;
    // The relative major of a minor key is a minor third above its tonic; going up a fifth moves one step
    // round the wheel, C major being 8B.
    let major_tonic = if minor { (key + 3) % PITCH_CLASSES } else { key };
    Some(((major_tonic * 7 + 7) % PITCH_CLASSES + 1, minor))
}

/// The Camelot code of a key (`8B`, `5A`, ...), "" when unknown.
pub fn camelot_code(key: i32) -> String {
    camelot(key).map_or_else(String::new, |(number, minor)| format!("{number}{}", if minor { 'A' } else { 'B' }))
}

/// The key of a Camelot code, -1 if none.
pub fn camelot_key(code: &str) -> i32 {
    (0..KEYS).find(|&key| camelot_code(key) == code).unwrap_or(-1)
}

/// The key as musicians write it (`C major`, `F# minor`), "" when unknown.
pub fn key_name(key: i32) -> String {
    if !(0..KEYS).contains(&key) {
        return String::new();
    }
    let mode = if key >= PITCH_CLASSES { "minor" } else { "major" };
    format!("{} {mode}", NOTES[(key % PITCH_CLASSES) as usize])
}

/// What a change of key is (the keys of `KEY_COSTS` of the Python radio, and "unknown").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyRelation {
    Same,
    Neighbour,
    Boost,
    Semitone,
    Down,
    Diagonal,
    Clash,
    Unknown,
}

impl KeyRelation {
    /// The name the API and the statistics give it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Neighbour => "neighbour",
            Self::Boost => "boost",
            Self::Semitone => "semitone",
            Self::Down => "down",
            Self::Diagonal => "diagonal",
            Self::Clash => "clash",
            Self::Unknown => "unknown",
        }
    }
}

/// Name the key change.
pub fn key_relation(current: i32, following: i32) -> KeyRelation {
    let (Some((now, now_minor)), Some((then, then_minor))) = (camelot(current), camelot(following)) else {
        return KeyRelation::Unknown;
    };
    let steps = (then - now).rem_euclid(12);
    if now_minor == then_minor {
        match steps {
            0 => KeyRelation::Same,
            1 | 11 => KeyRelation::Neighbour,
            2 => KeyRelation::Boost,
            7 => KeyRelation::Semitone,
            10 => KeyRelation::Down,
            _ => KeyRelation::Clash,
        }
    } else {
        match steps {
            0 => KeyRelation::Neighbour,
            1 | 11 => KeyRelation::Diagonal,
            _ => KeyRelation::Clash,
        }
    }
}

/// How good a key change is for the listener, roughly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyQuality {
    Good,
    Fair,
    Poor,
    Unknown,
}

pub fn key_quality(relation: KeyRelation) -> KeyQuality {
    match relation {
        KeyRelation::Same | KeyRelation::Neighbour => KeyQuality::Good,
        KeyRelation::Boost | KeyRelation::Semitone | KeyRelation::Down | KeyRelation::Diagonal => KeyQuality::Fair,
        KeyRelation::Clash => KeyQuality::Poor,
        KeyRelation::Unknown => KeyQuality::Unknown,
    }
}

/// Cost of a key change by the rules of harmonic mixing: the same key, one step round the wheel and the
/// relative key blend cleanly; two steps up lifts the energy, seven steps up raises the key by a semitone;
/// anything else clashes. A key that was not detected costs nothing.
pub fn key_cost(current: i32, following: i32) -> f64 {
    let cost = match key_relation(current, following) {
        KeyRelation::Same | KeyRelation::Unknown => 0.0,
        KeyRelation::Neighbour => KEY_NEIGHBOUR_COST,
        KeyRelation::Boost => KEY_BOOST_COST,
        KeyRelation::Semitone => KEY_SEMITONE_COST,
        KeyRelation::Down => KEY_DOWN_COST,
        KeyRelation::Diagonal => KEY_DIAGONAL_COST,
        KeyRelation::Clash => KEY_CLASH_COST,
    };
    KEY_COST_SCALE * cost
}

/// The quality of a key change by the name the statistics keep it under (`KeyRelation::name`); "unknown" for any
/// other name.
pub fn quality_of_name(name: &str) -> KeyQuality {
    match name {
        "same" | "neighbour" => KeyQuality::Good,
        "boost" | "semitone" | "down" | "diagonal" => KeyQuality::Fair,
        "clash" => KeyQuality::Poor,
        _ => KeyQuality::Unknown,
    }
}

/// How many key changes of each quality (`quality_counts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct QualityCounts {
    pub good: i64,
    pub fair: i64,
    pub poor: i64,
    pub unknown: i64,
}

/// Sum the counts of key relations up into the qualities.
pub fn quality_counts<'a>(by_key: impl IntoIterator<Item = (&'a str, i64)>) -> QualityCounts {
    let mut counts = QualityCounts::default();
    for (relation, number) in by_key {
        match quality_of_name(relation) {
            KeyQuality::Good => counts.good += number,
            KeyQuality::Fair => counts.fair += number,
            KeyQuality::Poor => counts.poor += number,
            KeyQuality::Unknown => counts.unknown += number,
        }
    }
    counts
}
