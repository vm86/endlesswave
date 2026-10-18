//! The fake backend, the types of a backend and the clocks. Tasks 3.0.1 to 3.0.3 of docs/rust-port-radio.md.
//!
//! The fake backend is run on the series of calls that the Python stand (`tools/radio_harness.py`) made, and must
//! answer the same and leave the same log. The files of the library (`tests/golden/radio/library/*.wav`) are not in git;
//! without them the test is skipped, unless `GOLDEN_REQUIRED` is set.

use radio_core::{
    backend::{
        AuthPrompt, Feedback, MusicBackend, Track,
        fake::{FakeBackend, FakeOptions, FakeTrack},
    },
    clock::{Clock, ManualClock, SystemClock},
};
use serde_json::Value;
use std::{collections::HashSet, fs, path::PathBuf, time::Duration};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/radio")
}

fn json(name: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(dir().join(name)).unwrap()).unwrap()
}

fn track(id: &str) -> Track {
    Track {
        track_id: id.to_owned(),
        title: "?".to_owned(),
        artists: vec![],
        codec: "wav".to_owned(),
        bitrate_in_kbps: 0,
        genre: String::new(),
        cover: String::new(),
        batch_id: String::new(),
        mood: String::new(),
        source: String::new(),
        native_id: String::new(),
    }
}

#[test]
fn a_track_of_python_is_read_and_written_back() {
    let library = json("library.json");
    let entries = library.as_array().unwrap();
    assert_eq!(entries.len(), 8);
    for entry in entries {
        // `dataclasses.asdict(Track)` has these fields and no others: the library entries have `file` besides.
        let mut asdict = entry.clone();
        asdict.as_object_mut().unwrap().remove("file");
        let track: Track = serde_json::from_value(asdict.clone()).unwrap();
        assert_eq!(serde_json::to_value(&track).unwrap(), asdict);
    }
    // A track kept by an older version has fewer fields.
    let old: Track =
        serde_json::from_str(r#"{"track_id":"1","title":"T","artists":["A","B"],"codec":"mp3"}"#)
            .unwrap();
    assert_eq!(
        (old.bitrate_in_kbps, old.genre.as_str(), old.mood.as_str()),
        (0, "", "")
    );
}

#[tokio::test]
async fn the_fake_backend_answers_and_logs_as_the_python_one_does() {
    let library = json("library.json");
    let files = dir().join("library");
    if !files.join("t0.wav").exists() {
        assert!(
            std::env::var_os("GOLDEN_REQUIRED").is_none(),
            "no library, run tools/gen_golden.py"
        );
        eprintln!("no library: skipped");
        return;
    }
    let tracks: Vec<FakeTrack> = library
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            let mut asdict = entry.clone();
            let file = files.join(
                asdict
                    .as_object_mut()
                    .unwrap()
                    .remove("file")
                    .unwrap()
                    .as_str()
                    .unwrap(),
            );
            FakeTrack {
                track: serde_json::from_value(asdict).unwrap(),
                file,
            }
        })
        .collect();
    let backend = FakeBackend::new(
        tracks,
        FakeOptions {
            supports_moods: true,
            fail_downloads: HashSet::from(["t5".to_owned()]),
            ..FakeOptions::default()
        },
    );
    assert!(backend.authorized() && backend.supports_moods());
    assert_eq!(backend.auth_prompt(), AuthPrompt::default());
    assert_eq!(backend.default_station(), "fake");
    assert!(backend.authorize(&Default::default()).await.is_err());

    let scratch = std::env::temp_dir().join(format!("radio-core-backend-{}", std::process::id()));
    fs::create_dir_all(&scratch).unwrap();
    let golden = json("backend.json");
    for step in golden["results"].as_array().unwrap() {
        match step["op"].as_str().unwrap() {
            "batch" => {
                let exclude: HashSet<String> = step["exclude"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect();
                let got: Vec<String> = backend
                    .station_batch("fake", &exclude)
                    .await
                    .unwrap()
                    .into_iter()
                    .map(|t| t.track_id)
                    .collect();
                let want: Vec<&str> = step["ids"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap())
                    .collect();
                assert_eq!(got, want, "batch excluding {exclude:?}");
            }
            "download" => {
                let id = step["id"].as_str().unwrap();
                let path = scratch.join(format!("{id}.wav"));
                let done = backend.download(&track(id), &path).await.is_ok();
                assert_eq!(done, step["ok"].as_bool().unwrap(), "download {id}");
                if done {
                    assert_eq!(
                        fs::metadata(&path).unwrap().len(),
                        step["size"].as_u64().unwrap()
                    );
                }
            }
            "feedback" => {
                let kind = match step["kind"].as_str().unwrap() {
                    "started" => Feedback::Started,
                    "finished" => Feedback::Finished,
                    _ => Feedback::Skipped,
                };
                backend
                    .feedback_track(
                        &track(step["id"].as_str().unwrap()),
                        kind,
                        "fake",
                        step["seconds"].as_f64().unwrap(),
                    )
                    .await
                    .unwrap();
            }
            "settings" => {
                let triple = |v: &Value| -> Vec<String> {
                    v.as_array()
                        .unwrap()
                        .iter()
                        .map(|s| s.as_str().unwrap().to_owned())
                        .collect()
                };
                let before = backend.station_settings("fake").await.unwrap();
                assert_eq!(vec![before.0, before.1, before.2], triple(&step["before"]));
                backend
                    .set_station_settings("fake", "calm", "default", "russian")
                    .await
                    .unwrap();
                let after = backend.station_settings("fake").await.unwrap();
                assert_eq!(vec![after.0, after.1, after.2], triple(&step["after"]));
            }
            other => panic!("{other}"),
        }
    }
    let want: Vec<String> = golden["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(backend.calls(), want);
}

#[test]
fn a_manual_clock_stands_until_it_is_moved() {
    let clock = ManualClock::new(100.0);
    assert_eq!(clock.monotonic(), 100.0);
    clock.advance(12.5);
    assert_eq!(clock.monotonic(), 112.5);
    clock.set(3.0);
    assert_eq!((clock.monotonic(), clock.wall()), (3.0, 3.0));
}

/// Tokio's paused time moves the system clock too, so a test can sit through hours at once.
#[tokio::test(start_paused = true)]
async fn the_system_clock_follows_the_time_of_tokio() {
    let clock = SystemClock::new();
    let before = clock.monotonic();
    tokio::time::sleep(Duration::from_secs(3600)).await;
    let passed = clock.monotonic() - before;
    assert!((passed - 3600.0).abs() < 1e-6, "{passed}");
    assert!(clock.wall() > 1.6e9, "the wall clock is the real one");
}
