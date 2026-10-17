//! The choice of the next track, the keys and the costs against the answers of the Python radio
//! (`tools/gen_golden.py`, `tests/golden/logic/choose.json`). Task 1.10 of docs/rust-port.md.

use radio_core::mix::{
    TrackAnalysis,
    choose::{self, Entry},
    key,
    plan::{self, Transition},
};
use serde::Deserialize;
use std::{collections::HashSet, fs};

#[derive(Deserialize)]
struct Key {
    key: i32,
    camelot: String,
    name: String,
}
#[derive(Deserialize)]
struct Relation {
    from: i32,
    to: i32,
    relation: String,
    quality: String,
    cost: f64,
}
#[derive(Deserialize)]
struct Sample {
    analysis: TrackAnalysis,
    energy: f64,
    mood: String,
}
#[derive(Deserialize)]
struct Arc {
    position: i64,
    target: f64,
    mood: String,
}
#[derive(Deserialize)]
struct Pair {
    current: TrackAnalysis,
    following: TrackAnalysis,
    speed: f64,
    section: Option<i64>,
    cost: f64,
    tempo_ratio: Option<f64>,
    kind: std::collections::HashMap<String, String>,
}
#[derive(Deserialize)]
struct PoolEntry {
    analysis: TrackAnalysis,
    artists: Vec<String>,
    mood: String,
}
#[derive(Deserialize)]
struct Scenario {
    current: Option<TrackAnalysis>,
    pool: Vec<PoolEntry>,
    history: Vec<Vec<String>>,
    position: i64,
    index: usize,
    cost: f64,
    steps: Vec<f64>,
}
#[derive(Deserialize)]
struct Golden {
    keys: Vec<Key>,
    relations: Vec<Relation>,
    tracks: Vec<Sample>,
    arc: Vec<Arc>,
    pairs: Vec<Pair>,
    scenarios: Vec<Scenario>,
}

fn golden() -> Golden {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/logic/choose.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn close(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() < 1e-9, "{what}: {got} against {want}");
}

#[test]
fn keys_have_the_codes_and_names_of_python() {
    for k in golden().keys {
        assert_eq!(key::camelot_code(k.key), k.camelot, "key {}", k.key);
        assert_eq!(key::key_name(k.key), k.name, "key {}", k.key);
        if !k.camelot.is_empty() {
            assert_eq!(key::camelot_key(&k.camelot), k.key);
        }
    }
    assert_eq!(key::camelot_key("13B"), -1);
}

#[test]
fn every_change_of_key_is_named_and_priced_as_python_does() {
    let quality = |q: key::KeyQuality| match q {
        key::KeyQuality::Good => "good",
        key::KeyQuality::Fair => "fair",
        key::KeyQuality::Poor => "poor",
        key::KeyQuality::Unknown => "unknown",
    };
    let relations = golden().relations;
    assert!(relations.len() > 700);
    for r in relations {
        let relation = key::key_relation(r.from, r.to);
        let what = format!("{} -> {}", r.from, r.to);
        assert_eq!(relation.name(), r.relation, "{what}");
        assert_eq!(quality(key::key_quality(relation)), r.quality, "{what}");
        close(key::key_cost(r.from, r.to), r.cost, &what);
    }
}

#[test]
fn energy_and_mood_of_a_track_and_the_arc_of_the_set() {
    let golden = golden();
    for (i, t) in golden.tracks.iter().enumerate() {
        close(choose::energy(&t.analysis), t.energy, &format!("energy of track {i}"));
        assert_eq!(choose::mood_of(&t.analysis), t.mood, "mood of track {i}");
    }
    for a in golden.arc {
        close(choose::arc_target(a.position), a.target, &format!("arc at {}", a.position));
        assert_eq!(choose::mood_for(a.position), a.mood, "mood at {}", a.position);
    }
}

#[test]
fn nearest_mood_takes_the_closest_allowed() {
    assert_eq!(choose::nearest_mood("fun", &[]), "fun");
    assert_eq!(choose::nearest_mood("fun", &["fun", "calm"]), "fun");
    assert_eq!(choose::nearest_mood("sad", &["fun", "active"]), "fun");
    assert_eq!(choose::nearest_mood("active", &["sad", "calm"]), "calm");
    // Unknown wanted: the first allowed one. A tie: the first of the two.
    assert_eq!(choose::nearest_mood("loud", &["calm", "fun"]), "calm");
    assert_eq!(choose::nearest_mood("calm", &["sad", "fun"]), "sad");
    assert_eq!(choose::mood_cost("calm", "active"), 4.0);
    assert_eq!(choose::mood_cost("sad", "active"), 0.0);
}

#[test]
fn a_pair_of_tracks_is_judged_as_python_judges_it() {
    for (i, p) in golden().pairs.iter().enumerate() {
        let what = format!("pair {i}");
        close(plan::transition_speed(p.current.bpm, &p.following), p.speed, &what);
        assert_eq!(plan::section_length(&p.current, &p.following), p.section, "{what}");
        close(choose::transition_cost(&p.current, &p.following), p.cost, &what);
        if let Some(ratio) = p.tempo_ratio {
            close(plan::tempo_ratio(p.current.bpm, p.following.bpm), ratio, &what);
        }
        let name = |t: Transition| t.name();
        for matched in [true, false] {
            let got = name(plan::choose_transition(&p.current, &p.following, matched, 0.0));
            // The file has Python's str(True) and str(False).
            assert_eq!(got, p.kind[if matched { "True" } else { "False" }], "{what}");
        }
    }
}

#[test]
fn choose_next_picks_what_python_picks() {
    let scenarios = golden().scenarios;
    assert!(scenarios.len() >= 80);
    for (n, s) in scenarios.iter().enumerate() {
        let pool: Vec<Entry> = s
            .pool
            .iter()
            .map(|e| Entry {
                analysis: e.analysis.clone(),
                artists: e.artists.iter().cloned().collect(),
                mood: e.mood.clone(),
            })
            .collect();
        let history: Vec<HashSet<String>> = s.history.iter().map(|h| h.iter().cloned().collect()).collect();
        let what = format!("scenario {n}");
        for (i, entry) in pool.iter().enumerate() {
            close(choose::step_cost(s.current.as_ref(), entry, &history, s.position), s.steps[i], &format!("{what}, step {i}"));
        }
        close(choose::best_queue_cost(s.current.as_ref(), &pool, &history, s.position), s.cost, &what);
        assert_eq!(choose::choose_next(s.current.as_ref(), &pool, &history, s.position), s.index, "{what}");
    }
}
