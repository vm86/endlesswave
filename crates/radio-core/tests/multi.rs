//! The MultiBackend against the Python one (`tools/gen_golden.py`, `tests/golden/radio/multi_scenarios.json`). Task 5.1
//! of docs/rust-port.md.
//!
//! Both run a scenario over fake backends that hold parts of the library (one with plain ids, one that steers the station
//! by the mood, one that is not signed in), with a clock set by hand, and after each step tell what they look like: what
//! they answer to a query, what each backend was asked, which changes were told. The two must be the same. The fakes copy
//! files of `tests/golden/radio/library/` (not in git); without them the test is skipped, unless `GOLDEN_REQUIRED` is set.

use radio_core::{
    api::BackendSet,
    backend::{
        Feedback, MusicBackend, Track,
        fake::{FakeBackend, FakeOptions, FakeTrack},
        multi::{BackendSpec, MultiBackend},
    },
    clock::ManualClock,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/radio")
}

fn values(step: &Value) -> HashMap<String, String> {
    step["values"].as_object().into_iter().flatten().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned())).collect()
}

fn first_difference(got: &Value, want: &Value, path: &str) -> Option<String> {
    match (got, want) {
        (Value::Object(a), Value::Object(b)) => a
            .keys()
            .chain(b.keys())
            .find_map(|key| first_difference(a.get(key).unwrap_or(&Value::Null), b.get(key).unwrap_or(&Value::Null), &format!("{path}.{key}"))),
        (Value::Array(a), Value::Array(b)) if a.len() == b.len() => a.iter().zip(b).enumerate().find_map(|(i, (x, y))| first_difference(x, y, &format!("{path}[{i}]"))),
        _ if got == want => None,
        _ => Some(format!("{path}: {got} against {want}")),
    }
}

fn query(multi: &MultiBackend) -> Value {
    let prompt = multi.auth_prompt();
    let mut unavailable = multi.unavailable();
    unavailable.sort();
    json!({
        "authorized": multi.authorized(),
        "prompt": {"url": prompt.url, "code": prompt.code, "fields": prompt.fields},
        "default_station": multi.default_station(),
        "supports_moods": multi.supports_moods(),
        "status": multi.status(),
        "unavailable": unavailable,
    })
}

/// How Python names what went wrong (the type before the colon).
fn named(kind: &str, message: &str) -> String {
    format!("{kind}: {message}")
}

#[tokio::test]
async fn the_multi_backend_answers_as_the_python_one_does() {
    let library_dir = dir().join("library");
    if !library_dir.join("t0.wav").exists() {
        assert!(std::env::var_os("GOLDEN_REQUIRED").is_none(), "no library, run tools/gen_golden.py");
        eprintln!("no library: skipped");
        return;
    }
    let library: Vec<Value> = serde_json::from_str(&fs::read_to_string(dir().join("library.json")).unwrap()).unwrap();
    let scenarios: Vec<Value> = serde_json::from_str(&fs::read_to_string(dir().join("multi_scenarios.json")).unwrap()).unwrap();
    let scratch = std::env::temp_dir().join(format!("radio-core-multi-{}", std::process::id()));
    fs::create_dir_all(&scratch).unwrap();

    for scenario in &scenarios {
        let name = scenario["name"].as_str().unwrap();
        let clock = Arc::new(ManualClock::new(1000.0));
        let mut fakes: HashMap<String, Arc<FakeBackend>> = HashMap::new();
        let mut specs = Vec::new();
        for entry in scenario["backends"].as_array().unwrap() {
            let id = entry["id"].as_str().unwrap().to_owned();
            let wanted: HashSet<&str> = entry["tracks"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
            let tracks: Vec<FakeTrack> = library
                .iter()
                .filter(|e| wanted.contains(e["track_id"].as_str().unwrap()))
                .map(|e| {
                    let mut asdict = e.clone();
                    let file = library_dir.join(asdict.as_object_mut().unwrap().remove("file").unwrap().as_str().unwrap());
                    FakeTrack { track: serde_json::from_value(asdict).unwrap(), file }
                })
                .collect();
            let fake = Arc::new(FakeBackend::new(
                tracks,
                FakeOptions {
                    batch_size: 2,
                    supports_moods: entry["supports_moods"].as_bool().unwrap_or(false),
                    default_station: format!("{id}-station"),
                    authorized: entry["authorized"].as_bool().unwrap_or(true),
                    ..FakeOptions::default()
                },
            ));
            fakes.insert(id.clone(), fake.clone());
            let configured = entry["configured"].as_bool().unwrap_or(false);
            let mut spec = BackendSpec::new(&id, entry["name"].as_str().unwrap(), move || fake.clone() as Arc<dyn MusicBackend>).configured(move || configured);
            if entry["plain_ids"].as_bool().unwrap_or(false) {
                spec = spec.plain_ids();
            }
            specs.push(spec);
        }
        let changes: Arc<Mutex<Vec<Vec<String>>>> = Arc::default();
        let sink = changes.clone();
        let enabled: Vec<String> = scenario["enabled"].as_array().into_iter().flatten().map(|v| v.as_str().unwrap().to_owned()).collect();
        let stations: HashMap<String, String> = scenario["stations"].as_object().into_iter().flatten().map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned())).collect();
        let multi = MultiBackend::new(
            specs,
            &enabled,
            scenario["auto"].as_bool().unwrap_or(true),
            stations,
            Some(Box::new(move |ids| sink.lock().unwrap().push(ids.to_vec()))),
            clock.clone(),
        );

        let mut tracks: HashMap<String, Track> = HashMap::new();
        let mut taken: HashMap<String, usize> = fakes.keys().map(|k| (k.clone(), 0)).collect();
        let mut reported = 0;
        for (n, (step, want)) in scenario["steps"].as_array().unwrap().iter().zip(scenario["results"].as_array().unwrap()).enumerate() {
            let mut got = json!({});
            let mut error: Option<String> = None;
            match step["op"].as_str().unwrap() {
                "enable" => error = multi.enable(step["id"].as_str().unwrap()).await.err().map(|m| named("ValueError", &m)),
                "disable" => error = multi.disable(step["id"].as_str().unwrap()).await.err().map(|m| named("ValueError", &m)),
                "batch" => {
                    let exclude: HashSet<String> = step["exclude"].as_array().into_iter().flatten().map(|v| v.as_str().unwrap().to_owned()).collect();
                    match multi.station_batch("ignored", &exclude).await {
                        Ok(batch) => {
                            got["tracks"] = serde_json::to_value(&batch).unwrap();
                            tracks.extend(batch.into_iter().map(|t| (t.track_id.clone(), t)));
                        }
                        Err(e) => error = Some(named("RuntimeError", &e.0)),
                    }
                }
                "download" => {
                    let path = scratch.join(format!("{name}-{}.wav", step["track"].as_str().unwrap().replace(':', "_")));
                    if let Err(e) = multi.download(&tracks[step["track"].as_str().unwrap()], &path).await {
                        error = Some(named(if e.0.starts_with("the backend of") { "RuntimeError" } else { "BackendError" }, &e.0));
                    }
                }
                "feedback" => {
                    let kind = match step["kind"].as_str().unwrap() {
                        "started" => Feedback::Started,
                        "finished" => Feedback::Finished,
                        _ => Feedback::Skipped,
                    };
                    if let Err(e) = multi.feedback_track(&tracks[step["track"].as_str().unwrap()], kind, "ignored", step["seconds"].as_f64().unwrap()).await {
                        error = Some(named("RuntimeError", &e.0));
                    }
                }
                "settings" => {
                    let (a, b, c) = multi.station_settings("ignored").await.unwrap();
                    got["settings"] = json!([a, b, c]);
                }
                "set_settings" => {
                    let s = |k: &str| step[k].as_str().unwrap().to_owned();
                    if let Err(e) = multi.set_station_settings("ignored", &s("mood"), &s("diversity"), &s("language")).await {
                        error = Some(named("BackendError", &e.0));
                    }
                }
                "advance" => clock.advance(step["seconds"].as_f64().unwrap()),
                "set" => {
                    let fake = &fakes[step["backend"].as_str().unwrap()];
                    if let Some(failing) = step["failing"].as_bool() {
                        fake.set_failing(failing);
                    }
                    if let Some(authorized) = step["authorized"].as_bool() {
                        fake.set_authorized(authorized);
                    }
                }
                "authorize" => error = multi.authorize(&values(step)).await.err().map(|e| named("AuthError", &e.0)),
                "authorize_backend" => error = multi.authorize_backend(step["id"].as_str().unwrap(), &values(step)).await.err().map(|e| named("AuthError", &e.0)),
                "start_auth" => multi.start_auth(),
                "persist" => multi.persist(),
                "wait_authorized" => {
                    let waited = tokio::time::timeout(Duration::from_millis(300), multi.wait_authorized()).await;
                    got["waited"] = if waited.is_ok() { "returned" } else { "blocked" }.into();
                }
                "query" => {}
                other => panic!("{other}"),
            }
            if let Some(error) = error {
                got["error"] = error.into();
            }
            tokio::task::yield_now().await;
            let mut snapshot = query(&multi);
            let mut calls = serde_json::Map::new();
            for (id, fake) in &fakes {
                let all = fake.calls();
                calls.insert(id.clone(), json!(all[taken[id]..]));
                taken.insert(id.clone(), all.len());
            }
            snapshot["calls"] = Value::Object(calls);
            let all_changes = changes.lock().unwrap().clone();
            snapshot["changes"] = json!(all_changes[reported..]);
            reported = all_changes.len();
            got["snapshot"] = snapshot;
            if let Some(difference) = first_difference(&got, want, "") {
                panic!("{name}, step {n} ({step}): {difference}");
            }
        }
        println!("{name}: {} steps the same", scenario["steps"].as_array().unwrap().len());
    }
}
