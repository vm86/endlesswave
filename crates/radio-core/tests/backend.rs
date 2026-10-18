//! The types of a backend and the clocks. Tasks 3.0.1 to 3.0.3 of docs/rust-port-radio.md.

use radio_core::{
    backend::Track,
    clock::{Clock, ManualClock, SystemClock},
};
use serde_json::Value;
use std::{fs, path::PathBuf, time::Duration};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/radio")
}

fn json(name: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(dir().join(name)).unwrap()).unwrap()
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
