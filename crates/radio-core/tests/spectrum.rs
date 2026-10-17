//! The spectrum against the Python radio's (`tools/gen_golden.py`, `tests/golden/spectrum/`). Task 3.5 of
//! docs/rust-port.md. The mix (`mix.f32`) is not in git; without it the analyzer test is skipped, unless
//! `GOLDEN_REQUIRED` is set.

use radio_core::spectrum::{Frame, SpectrumAnalyzer, SpectrumFeed, StartPositions, sse_frames, sse_start};
use serde_json::Value;
use std::{fs, path::PathBuf, time::Duration};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/spectrum")
}

fn golden() -> Value {
    serde_json::from_str(&fs::read_to_string(dir().join("expected.json")).unwrap()).unwrap()
}

fn f(value: &Value) -> f64 {
    value.as_f64().unwrap()
}

fn bands(value: &Value) -> [f64; 12] {
    let list: Vec<f64> = value.as_array().unwrap().iter().map(f).collect();
    list.try_into().expect("twelve bands")
}

#[test]
fn the_analyzer_gives_the_bands_python_gives() {
    let Ok(bytes) = fs::read(dir().join("mix.f32")) else {
        assert!(std::env::var_os("GOLDEN_REQUIRED").is_none(), "no mix, run tools/gen_golden.py");
        eprintln!("no mix: skipped");
        return;
    };
    let mix: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
    let golden = golden();
    let mut analyzer = SpectrumAnalyzer::new();
    let (mut place, mut compared, mut worst_band, mut flips) = (0, 0, 0.0f64, 0);
    for (n, feed) in golden["feeds"].as_array().unwrap().iter().enumerate() {
        let size = feed["size"].as_u64().unwrap() as usize;
        let frames = analyzer.feed(&mix[place * 2..(place + size) * 2]);
        place += size;
        assert_eq!(analyzer.position(), f(&feed["position"]), "feed {n}: position");
        let want = feed["frames"].as_array().unwrap();
        assert_eq!(frames.len(), want.len(), "feed {n}: number of frames");
        for (got, want) in frames.iter().zip(want) {
            assert_eq!(got.t, f(&want["t"]), "feed {n}: t");
            for (g, w) in got.bands.iter().zip(bands(&want["b"])) {
                worst_band = worst_band.max((g - w).abs());
            }
            // The text has the bands to 3 places: of the same numbers the same text, but for the rare one that
            // lies on the edge between two roundings.
            let (got_json, want_json) = (got.to_json(), want["json"].as_str().unwrap());
            if got_json != want_json {
                flips += 1;
                let (a, b): (Value, Value) = (serde_json::from_str(&got_json).unwrap(), serde_json::from_str(want_json).unwrap());
                assert_eq!(a["t"], b["t"]);
                for (x, y) in a["b"].as_array().unwrap().iter().zip(b["b"].as_array().unwrap()) {
                    assert!((f(x) - f(y)).abs() < 0.0011, "feed {n}: {got_json} against {want_json}");
                }
            }
            compared += 1;
        }
    }
    assert_eq!(place * 2, mix.len());
    assert!(compared > 150, "{compared} frames");
    println!("{compared} frames, the bands differ by {worst_band:e} at most, {flips} texts on a rounding edge");
    assert!(worst_band < 1e-4, "the bands differ by {worst_band}");
    assert!(flips <= 2, "{flips} texts differ");
}

#[test]
fn the_text_of_a_frame_is_the_pythons() {
    for case in golden()["formats"].as_array().unwrap() {
        let frame = Frame { t: f(&case["t"]), bands: bands(&case["b"]) };
        assert_eq!(frame.to_json(), case["json"].as_str().unwrap(), "t = {}", case["t"]);
    }
}

#[test]
fn server_sent_events() {
    let frames = [Frame { t: 1.5, bands: [0.25; 12] }, Frame { t: 1.533, bands: [0.0; 12] }];
    let text = sse_frames(&frames);
    assert_eq!(text, format!("data: {}\n\ndata: {}\n\n", frames[0].to_json(), frames[1].to_json()));
    assert_eq!(sse_start(None), "event: start\ndata: {\"position\": null}\n\n");
    assert_eq!(sse_start(Some(12.5)), "event: start\ndata: {\"position\": 12.5}\n\n");
}

fn batch(from: f64, count: usize) -> Vec<Frame> {
    (0..count).map(|i| Frame { t: from + i as f64 * 0.033, bands: [i as f64; 12] }).collect()
}

#[tokio::test]
async fn the_feed_keeps_frames_for_who_joins_and_drops_for_who_lags() {
    let golden = golden();
    let want = &golden["feed"];
    let feed = SpectrumFeed::new();
    let mut clock = 0.0;
    for size in want["batches"].as_array().unwrap() {
        let size = size.as_u64().unwrap() as usize;
        feed.publish(batch(clock, size));
        clock += size as f64 * 0.033;
    }
    for case in want["subscribed"].as_array().unwrap() {
        let (_watcher, backlog) = feed.subscribe(case["since"].as_f64());
        let ts: Vec<f64> = backlog.iter().map(|f| f.t).collect();
        let expected: Vec<f64> = case["ts"].as_array().unwrap().iter().map(f).collect();
        assert_eq!(ts, expected, "since {}", case["since"]);
    }
    let (watcher, _) = feed.subscribe(None);
    for n in 0..100 {
        feed.publish(vec![Frame { t: f64::from(n), bands: [0.0; 12] }]);
    }
    let kept: Vec<f64> = std::iter::from_fn(|| watcher.try_recv()).map(|b| b[0].t).collect();
    let expected: Vec<f64> = want["kept"].as_array().unwrap().iter().map(f).collect();
    assert_eq!(kept, expected);
    assert_eq!(kept.len(), 64);
    feed.reset();
    let (_watcher, after) = feed.subscribe(Some(0.0));
    assert_eq!(after.len() as u64, want["after_reset"].as_u64().unwrap());
}

#[tokio::test]
async fn a_page_gets_what_is_published_after_it_joined() {
    let feed = SpectrumFeed::new();
    let (watcher, backlog) = feed.subscribe(None);
    assert!(backlog.is_empty());
    let reader = tokio::spawn(async move { watcher.recv().await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    feed.publish(batch(1.0, 2));
    let got = tokio::time::timeout(Duration::from_secs(2), reader).await.expect("got it").unwrap();
    assert_eq!(got.len(), 2);
}

#[tokio::test]
async fn the_start_of_a_listener_is_remembered_and_waited_for() {
    let want = golden()["starts"].clone();
    let positions = StartPositions::new();
    for i in 0..300 {
        positions.set(&format!("n{i}"), f64::from(i));
    }
    positions.set("n100", 1000.5);
    for i in 300..330 {
        positions.set(&format!("n{i}"), f64::from(i));
    }
    for i in 0..340 {
        let name = format!("n{i}");
        assert_eq!(positions.get(&name, Duration::ZERO).await, want[&name].as_f64(), "{name}");
    }

    let positions = std::sync::Arc::new(StartPositions::new());
    let waiting = {
        let positions = positions.clone();
        tokio::spawn(async move { positions.get("page", Duration::from_secs(5)).await })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    positions.set("page", 7.25);
    assert_eq!(tokio::time::timeout(Duration::from_secs(2), waiting).await.expect("woken").unwrap(), Some(7.25));
    assert_eq!(positions.get("nobody", Duration::from_millis(30)).await, None);
}
