//! The cache against the Python radio's (`tools/gen_golden.py` makes databases with it; `tools/check_cache.py` reads
//! one that Rust wrote). Stage 2 of docs/rust-port.md.
//!
//! The databases (`tests/golden/cache/*.db`) are not in git; a test without them is skipped, unless
//! `GOLDEN_REQUIRED` is set. The Python side of the second direction runs when `YAMUSIC_RADIO` (the checkout) and
//! `RADIO_PYTHON` (a Python with the radio's libraries) are set.

use radio_core::{
    cache::{NewTransition, TrackCache},
    mix::TrackAnalysis,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Deserialize)]
struct Analysed {
    track_id: String,
    codec: String,
    analysis: Option<TrackAnalysis>,
    genre: Option<String>,
}
#[derive(Deserialize)]
struct Quality {
    good: i64,
    fair: i64,
    poor: i64,
    unknown: i64,
}
#[derive(Deserialize)]
struct Stats {
    total: i64,
    beat_matched: i64,
    by_type: BTreeMap<String, i64>,
    by_key: BTreeMap<String, i64>,
    by_effect: BTreeMap<String, i64>,
    by_quality: Quality,
}
#[derive(Deserialize)]
struct Main {
    key_detector: String,
    loudness_meter: String,
    analyses: Vec<Analysed>,
    settings: BTreeMap<String, Value>,
    stats: Stats,
    key_pairs: Vec<(String, String, i64)>,
    known_genres: Vec<(String, i64)>,
    recently_played: BTreeMap<String, Vec<String>>,
}
#[derive(Deserialize)]
struct Old {
    tracks: Vec<String>,
    transitions: Vec<String>,
    stats: Stats,
    key_pairs: Vec<(String, String, i64)>,
}
#[derive(Deserialize)]
struct Expected {
    main: Main,
    prune: Vec<String>,
    old: Old,
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/cache")
}

fn expected() -> Expected {
    serde_json::from_str(&fs::read_to_string(dir().join("expected.json")).unwrap()).unwrap()
}

/// A copy of a database of Python's to open (opening may change it).
fn copy_of(name: &str) -> Option<PathBuf> {
    let source = dir().join(name);
    if !source.exists() {
        assert!(std::env::var_os("GOLDEN_REQUIRED").is_none(), "{name}: run tools/gen_golden.py");
        eprintln!("{name}: not made, skipped");
        return None;
    }
    let copy = scratch(name);
    fs::copy(source, &copy).unwrap();
    Some(copy)
}

fn scratch(name: &str) -> PathBuf {
    let folder = std::env::temp_dir().join(format!("radio-core-cache-{}", std::process::id()));
    fs::create_dir_all(&folder).unwrap();
    folder.join(name)
}

fn map(pairs: Vec<(String, i64)>) -> BTreeMap<String, i64> {
    pairs.into_iter().collect()
}

fn same_stats(got: &radio_core::cache::TransitionStats, want: &Stats) {
    assert_eq!((got.total, got.beat_matched), (want.total, want.beat_matched));
    // The counts are equal; of equal counts the order is up to SQLite.
    assert_eq!(map(got.by_type.clone()), want.by_type);
    assert_eq!(map(got.by_key.clone()), want.by_key);
    assert_eq!(map(got.by_effect.clone()), want.by_effect);
    let q = &got.by_quality;
    assert_eq!((q.good, q.fair, q.poor, q.unknown), (want.by_quality.good, want.by_quality.fair, want.by_quality.poor, want.by_quality.unknown));
    let counts: Vec<i64> = got.by_type.iter().map(|(_, n)| *n).collect();
    assert!(counts.windows(2).all(|w| w[0] >= w[1]), "the most common first: {counts:?}");
}

#[test]
fn a_cache_of_the_python_radio_reads_the_same() {
    let Some(path) = copy_of("python.db") else { return };
    let want = expected().main;
    let cache = TrackCache::open(&path).unwrap();

    for a in &want.analyses {
        let got = cache.get_analysis_by(&a.track_id, &a.codec, &want.key_detector, &want.loudness_meter);
        assert_eq!(got, a.analysis, "analysis of {} {}", a.track_id, a.codec);
        assert_eq!(cache.get_genre(&a.track_id, &a.codec), a.genre, "genre of {} {}", a.track_id, a.codec);
        // Made by another detector than this build has: the track is analyzed again, as Python does.
        if want.key_detector != radio_core::mix::key_detector() {
            assert_eq!(cache.get_analysis(&a.track_id, &a.codec), None);
        }
    }
    assert!(want.analyses.iter().any(|a| a.analysis.is_none()) && want.analyses.iter().any(|a| a.analysis.is_some()));
    for (key, value) in &want.settings {
        assert_eq!(cache.get_setting(key).unwrap_or(Value::Null), *value, "setting {key}");
    }
    same_stats(&cache.transition_stats().unwrap(), &want.stats);
    assert_eq!(cache.key_pairs().unwrap(), want.key_pairs);
    assert_eq!(cache.known_genres().unwrap(), want.known_genres);
    for (limit, ids) in &want.recently_played {
        let got: BTreeSet<String> = cache.recently_played(limit.parse().unwrap()).unwrap().into_iter().collect();
        assert_eq!(got, ids.iter().cloned().collect::<BTreeSet<_>>(), "played, limit {limit}");
    }
}

#[test]
fn the_oldest_rows_go_first_as_in_python() {
    let Some(_) = copy_of("prune.db") else { return };
    let want = expected().prune;
    let path = scratch("rust-prune.db");
    let _ = fs::remove_file(&path);
    let cache = TrackCache::open(&path).unwrap().with_max_rows(5);
    let analysis = TrackAnalysis { bpm: 120.0, head_beat: 1.0, tail_beat: 2.0, start: 0, end: 10, loudness_db: -10.0, intro_end: -1, outro_start: -1, key: 3 };
    for i in 0..9 {
        cache.put_analysis(&format!("p:{i}"), "mp3", "P", &analysis, "").unwrap();
        // the clock of SQLite rows is a float of seconds: let it tick
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let kept: Vec<String> = (0..9).map(|i| format!("p:{i}")).filter(|id| cache.get_genre(id, "mp3").is_some()).collect();
    assert_eq!(kept, want);
}

#[test]
fn a_cache_of_the_first_version_gets_the_columns_it_lacks() {
    let Some(path) = copy_of("old.db") else { return };
    let want = expected().old;
    let cache = TrackCache::open(&path).unwrap();
    let columns = |table: &str| -> Vec<String> {
        let db = rusqlite::Connection::open(&path).unwrap();
        let mut statement = db.prepare(&format!("PRAGMA table_info({table})")).unwrap();
        statement.query_map([], |row| row.get(1)).unwrap().map(Result::unwrap).collect()
    };
    assert_eq!(columns("tracks"), want.tracks);
    assert_eq!(columns("transitions"), want.transitions);
    same_stats(&cache.transition_stats().unwrap(), &want.stats);
    assert_eq!(cache.key_pairs().unwrap(), want.key_pairs);
    assert_eq!(cache.get_genre("nothing", "mp3"), None);
}

fn sample(i: usize) -> TrackAnalysis {
    TrackAnalysis {
        bpm: 100.5 + i as f64,
        head_beat: 12033.0 + i as f64,
        tail_beat: 900_000.5,
        start: i as i64,
        end: 10_000_000,
        loudness_db: -9.25 - i as f64 / 7.0,
        intro_end: if i % 2 == 0 { 400_000 } else { -1 },
        outro_start: if i % 2 == 0 { 9_000_000 } else { -1 },
        key: if i == 3 { -1 } else { (i * 5 % 24) as i32 },
    }
}

#[test]
fn a_cache_that_rust_wrote_is_read_by_the_python_radio() {
    let (Some(radio), Some(python)) = (std::env::var_os("YAMUSIC_RADIO"), std::env::var_os("RADIO_PYTHON")) else {
        eprintln!("YAMUSIC_RADIO and RADIO_PYTHON are not set: skipped");
        return;
    };
    let path = scratch("rust.db");
    let _ = fs::remove_file(&path);
    let cache = TrackCache::open(&path).unwrap();
    // As the Python radio with Essentia made them, so that it takes them.
    for i in 0..10 {
        cache.put_analysis_by(&format!("r:{i}"), "mp3", &format!("Song {i}"), &sample(i), ["house", "", "techno"][i % 3], "essentia", "ebu r128").unwrap();
    }
    cache.put_analysis_by("r:0", "flac", "Song 0", &sample(0), "dub", "chroma profiles", "ebu r128").unwrap();
    cache.set_genre("r:2", "mp3", "электро").unwrap();
    for i in [4, 1, 7] {
        cache.mark_played(&format!("r:{i}"), "mp3").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    cache.put_setting("moods", &json!(["calm", "fun"])).unwrap();
    cache.put_setting("text", &json!("тест ✓")).unwrap();
    cache.put_setting("gone", &json!(1)).unwrap();
    cache.delete_setting("gone").unwrap();
    for n in 0..12 {
        cache
            .add_transition(&NewTransition {
                kind: ["bass swap", "eq blend", "filter sweep"][n % 3],
                beat_matched: n % 2 == 0,
                key_relation: ["same", "clash", "boost", "unknown"][n % 4],
                from_track: "r:0",
                to_track: "r:1",
                from_key: ["8A", "9B", ""][n % 3],
                to_key: ["8A", "5A"][n % 2],
                effect: ["none", "drop", "echo"][n % 3],
            })
            .unwrap();
    }
    drop(cache);

    let output = Command::new(python).args(["-I", "tools/check_cache.py"]).arg(&path).env("YAMUSIC_RADIO", radio).current_dir(env!("CARGO_MANIFEST_DIR")).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let seen: Value = serde_json::from_slice(&output.stdout).unwrap();

    for a in seen["analyses"].as_array().unwrap() {
        let (id, codec) = (a["track_id"].as_str().unwrap(), a["codec"].as_str().unwrap());
        let i: usize = id[2..].parse().unwrap();
        // r:3 has no key: Python (with Essentia) makes it again; r:0 flac was written by another detector.
        let analysed = !(i == 3 || codec == "flac");
        let want = if analysed { serde_json::to_value(sample(i)).unwrap() } else { Value::Null };
        assert_eq!(a["analysis"], want, "{id} {codec}");
    }
    let genre = |id: &str, codec: &str| seen["analyses"].as_array().unwrap().iter().find(|a| a["track_id"] == id && a["codec"] == codec).unwrap()["genre"].clone();
    assert_eq!(genre("r:2", "mp3"), "электро");
    assert_eq!(genre("r:0", "flac"), "dub");
    assert_eq!(seen["settings"], json!({"moods": ["calm", "fun"], "text": "тест ✓", "gone": null}));
    assert_eq!(seen["stats"]["total"], 12);
    assert_eq!(seen["stats"]["beat_matched"], 6);
    assert_eq!(seen["stats"]["by_effect"], json!({"none": 4, "drop": 4, "echo": 4}));
    assert_eq!(seen["stats"]["by_quality"], json!({"good": 3, "fair": 3, "poor": 3, "unknown": 3}));
    assert_eq!(seen["recently_played"], json!(["r:1", "r:4", "r:7"]));
    let rust_pairs: Vec<Value> = TrackCache::open(&path).unwrap().key_pairs().unwrap().into_iter().map(|(a, b, n)| json!([a, b, n])).collect();
    assert_eq!(Value::Array(rust_pairs), seen["key_pairs"]);
    let _: &Path = &path;
}
