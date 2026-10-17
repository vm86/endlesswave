//! The whole Radio against the Python one (`tools/gen_golden.py`, `tests/golden/radio/radio_scenarios.json`). Stage 3.1 of
//! docs/rust-port-radio.md.
//!
//! A scenario is a list of steps (fill the pool, make a deck, tell the radio a deck began, move the clock, call a
//! method). Both radios run it, on the same fake backend and the same cache of analyses, and after each step tell what
//! they look like: the queue, the pool, the files, the calls to the backend, the statistics. The two must be the same.
//!
//! The analyses come from the cache that the Python stand made (`primed.db`), so this needs the build with Essentia, whose
//! key and loudness the cache holds. The tracks (`library/*.wav`) and the cache are not in git; without them, or ffmpeg,
//! the test is skipped, unless `GOLDEN_REQUIRED` is set.

use radio_core::{
    backend::{
        MusicBackend,
        fake::{FakeBackend, FakeOptions, FakeTrack},
    },
    cache::TrackCache,
    clock::ManualClock,
    mix::{effects::Effect, plan::Transition},
    pipeline::{Deck, DeckSource},
    radio::{Radio, RadioConfig, RadioError},
};
use serde_json::{Value, json};
use std::{collections::HashMap, fs, path::PathBuf, sync::Arc};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/radio")
}

/// The first place where two values differ, for a message.
fn first_difference(got: &Value, want: &Value, path: &str) -> Option<String> {
    match (got, want) {
        (Value::Object(a), Value::Object(b)) => {
            for key in a.keys().chain(b.keys()) {
                let (x, y) = (a.get(key).unwrap_or(&Value::Null), b.get(key).unwrap_or(&Value::Null));
                if let Some(found) = first_difference(x, y, &format!("{path}.{key}")) {
                    return Some(found);
                }
            }
            None
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Some(format!("{path}: {} items against {}: {got} / {want}", a.len(), b.len()));
            }
            a.iter().zip(b).enumerate().find_map(|(i, (x, y))| first_difference(x, y, &format!("{path}[{i}]")))
        }
        _ if got == want => None,
        _ => Some(format!("{path}: {got} against {want}")),
    }
}

fn deck_info(deck: &Deck) -> Value {
    json!({
        "title": deck.title.to_string(),
        "bpm": deck.analysis.bpm,
        "key": deck.analysis.key,
        "beat_matched": deck.beat_matched,
        "speed": deck.speed,
        "generation": deck.generation,
        "mood": deck.mood,
    })
}

fn error_name(error: &RadioError) -> &'static str {
    match error {
        RadioError::SkipRefused(_) => "SkipRefusedError",
        RadioError::QueueChangeRefused(_) => "QueueChangeRefusedError",
        RadioError::UnknownQueueTrack(_) => "UnknownQueueTrackError",
        RadioError::Invalid(_) => "ValueError",
    }
}

fn snapshot(radio: &Radio, backend: &FakeBackend, workdir: &std::path::Path, taken: &mut usize) -> Value {
    let calls = backend.calls();
    let new: Vec<String> = calls[*taken..].to_vec();
    *taken = calls.len();
    let mut files: Vec<String> = fs::read_dir(workdir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    files.sort();
    let settings: serde_json::Map<String, Value> = ["playlist", "queue_order", "genres", "blocked_genres", "moods", "language", "station_settings"]
        .into_iter()
        .map(|key| (key.to_owned(), radio.setting(key).unwrap_or(Value::Null)))
        .collect();
    json!({
        "queue": radio.queue(2, 15),
        "pool": radio.pool().iter().map(|c| c.title.clone()).collect::<Vec<_>>(),
        "files": files,
        "generation": radio.generation(),
        "calls": new,
        "stats": radio.transition_stats()["session"],
        "key_pairs": radio.key_pair_stats()["session"],
        "blends": radio.recent_blends(),
        "order": radio.order(),
        "settings": settings,
    })
}

#[tokio::test]
async fn the_radio_looks_after_each_step_as_the_python_radio_does() {
    let skip = |why: &str| {
        assert!(std::env::var_os("GOLDEN_REQUIRED").is_none(), "{why}");
        eprintln!("{why}: skipped");
    };
    if !cfg!(feature = "essentia") {
        return skip("the cache of the stand holds the analyses of Essentia: build with --features essentia");
    }
    let library_dir = dir().join("library");
    let primed = dir().join("primed.db");
    if !library_dir.join("t0.wav").exists() || !primed.exists() {
        return skip("no library, run tools/gen_golden.py");
    }
    if !std::process::Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success()) {
        return skip("no ffmpeg");
    }
    let library: Vec<Value> = serde_json::from_str(&fs::read_to_string(dir().join("library.json")).unwrap()).unwrap();
    let scenarios: Vec<Value> = serde_json::from_str(&fs::read_to_string(dir().join("radio_scenarios.json")).unwrap()).unwrap();
    let scratch = std::env::temp_dir().join(format!("radio-core-radio-{}", std::process::id()));
    fs::create_dir_all(&scratch).unwrap();

    for scenario in &scenarios {
        let name = scenario["name"].as_str().unwrap();
        let options = &scenario["backend"];
        let tracks: Vec<FakeTrack> = library
            .iter()
            .map(|entry| {
                let mut asdict = entry.clone();
                let file = library_dir.join(asdict.as_object_mut().unwrap().remove("file").unwrap().as_str().unwrap());
                FakeTrack { track: serde_json::from_value(asdict).unwrap(), file }
            })
            .collect();
        let backend = Arc::new(FakeBackend::new(
            tracks,
            FakeOptions {
                batch_size: options["batch_size"].as_u64().unwrap_or(3) as usize,
                supports_moods: options["supports_moods"].as_bool().unwrap_or(false),
                fail_downloads: options["fail_downloads"].as_array().into_iter().flatten().map(|v| v.as_str().unwrap().to_owned()).collect(),
                ..FakeOptions::default()
            },
        ));
        let cache_path = scratch.join(format!("{name}.db"));
        fs::copy(&primed, &cache_path).unwrap();
        let workdir = scratch.join(format!("{name}-work"));
        fs::create_dir_all(&workdir).unwrap();
        let clock = Arc::new(ManualClock::new(1000.0));
        let make_radio = || {
            Radio::new(
                backend.clone() as Arc<dyn MusicBackend>,
                workdir.clone(),
                RadioConfig {
                    pool_size: scenario["pool_size"].as_u64().unwrap_or(4) as usize,
                    mood_arc: scenario["mood_arc"].as_bool().unwrap_or(false),
                    keep_playlist: scenario["keep_playlist"].as_bool().unwrap_or(false),
                    ..RadioConfig::default()
                },
                Some(TrackCache::open(&cache_path).unwrap()),
                clock.clone(),
            )
        };
        let mut radio = make_radio();

        let mut decks: HashMap<String, Deck> = HashMap::new();
        let mut taken = 0;
        let results = scenario["results"].as_array().unwrap();
        for (n, (step, want)) in scenario["steps"].as_array().unwrap().iter().zip(results).enumerate() {
            let mut got = json!({});
            match step["op"].as_str().unwrap() {
                "fill" => radio.fill(step["size"].as_u64().map(|s| s as usize)).await.unwrap(),
                "next_deck" => {
                    let after = step["after"].as_str().map(|a| decks[a].analysis.clone());
                    let deck = Radio::next_deck(&radio, after.as_ref()).await.unwrap();
                    got["deck"] = deck_info(&deck);
                    decks.insert(step["into"].as_str().unwrap().to_owned(), deck);
                }
                "started" => DeckSource::started(&*radio, &decks[step["deck"].as_str().unwrap()]),
                "finished" => DeckSource::finished(&*radio, &decks[step["deck"].as_str().unwrap()]),
                "transition" => {
                    let kind = [Transition::Crossfade, Transition::BassSwap, Transition::EqBlend, Transition::FilterSweep]
                        .into_iter()
                        .find(|k| k.name() == step["kind"].as_str().unwrap())
                        .unwrap();
                    let effect = [Effect::None, Effect::Drop, Effect::Echo].into_iter().find(|e| e.name() == step["effect"].as_str().unwrap()).unwrap();
                    DeckSource::transition(&*radio, kind, &decks[step["current"].as_str().unwrap()], &decks[step["following"].as_str().unwrap()], effect, None);
                }
                "advance" => clock.advance(step["seconds"].as_f64().unwrap()),
                "restart" => {
                    // the program is stopped and started again: the files and the cache stay
                    drop(radio);
                    radio = make_radio();
                    decks.clear();
                }
                "call" => {
                    let args = step["args"].as_array().cloned().unwrap_or_default();
                    let text = |i: usize| args[i].as_str().unwrap().to_owned();
                    let list = |i: usize| -> Vec<String> { args[i].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect() };
                    let outcome: Result<(), RadioError> = match step["method"].as_str().unwrap() {
                        "pause" => Ok(radio.pause()),
                        "resume" => Ok(radio.resume()),
                        "hold" => Ok(DeckSource::hold(&*radio, args[0].as_bool().unwrap())),
                        "request_skip" => radio.request_skip(),
                        "skip_taken" => Ok(DeckSource::skip_taken(&*radio)),
                        "refresh_queue" => Ok(radio.refresh_queue()),
                        "queue_order" => radio.queue_order(list(0)),
                        "queue_move" => radio.queue_move(&text(0), args[1].as_i64().unwrap()),
                        "queue_remove" => radio.queue_remove(&text(0)),
                        "queue_play" => radio.queue_play(&text(0)),
                        "set_wanted_genres" => Ok(radio.set_wanted_genres(list(0))),
                        "set_blocked_genres" => Ok(radio.set_blocked_genres(list(0))),
                        "genres_info" => {
                            got["returned"] = radio.genres_info();
                            Ok(())
                        }
                        "restore_playlist" => {
                            got["returned"] = radio.restore_playlist().into();
                            Ok(())
                        }
                        other => panic!("the method {other} is not run yet"),
                    };
                    if let Err(error) = outcome {
                        got["error"] = error_name(&error).into();
                    }
                }
                other => panic!("{other}"),
            }
            radio.wait_feedback().await;
            got["snapshot"] = snapshot(&radio, &backend, &workdir, &mut taken);
            if let Some(difference) = first_difference(&got, want, "") {
                panic!("{name}, step {n} ({}): {difference}", step);
            }
        }
        println!("{name}: {} steps the same", results.len());
    }
}
