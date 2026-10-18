//! What a listener of the streams and of the spectrum receives from the server. Task 4.4 of docs/rust-port.md.
//!

use axum::{Router, body::Body, http::Request};
use bytes::Bytes;
use http_body_util::BodyExt as _;
use radio_core::{
    api::{Api, router},
    backend::{
        MusicBackend,
        fake::{FakeBackend, FakeOptions},
    },
    clock::ManualClock,
    encode::Streams,
    radio::{Radio, RadioConfig},
    spectrum::{Frame, SpectrumFeed, StartPositions},
    stream::broadcaster::{Broadcaster, Config},
};
use serde_json::Value;
use std::{fs, path::PathBuf, sync::Arc, time::Duration};
use tower::ServiceExt as _;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/stream")
}

fn golden() -> Value {
    serde_json::from_str(&fs::read_to_string(dir().join("http_streams.json")).unwrap()).unwrap()
}

/// A radio with nothing to play: the routes of the streams do not need one.
fn radio() -> Arc<Radio> {
    let workdir = std::env::temp_dir().join(format!("radio-core-http-{}", std::process::id()));
    fs::create_dir_all(&workdir).unwrap();
    Radio::new(
        Arc::new(FakeBackend::new(Vec::new(), FakeOptions::default())) as Arc<dyn MusicBackend>,
        workdir,
        RadioConfig::default(),
        None,
        Arc::new(ManualClock::new(0.0)),
    )
}

fn frames_of(first: usize, count: usize) -> Vec<Frame> {
    (0..count)
        .map(|i| {
            let n = first + i;
            Frame {
                t: n as f64 * 0.033,
                bands: [(n % 100) as f64 / 100.0; 12],
            }
        })
        .collect()
}

struct World {
    streams: Streams,
    feed: Arc<SpectrumFeed>,
    app: Router,
}

impl World {
    fn new() -> Self {
        let streams = Streams {
            mp3: Broadcaster::new(Config::mp3()),
            flac: Some(Broadcaster::new(Config::flac())),
            ogg: Some(Broadcaster::new(Config::ogg())),
        };
        let feed = SpectrumFeed::new();
        let app = router(Api {
            radio: radio(),
            backends: None,
            streams: streams.clone(),
            spectrum: Some(feed.clone()),
            starts: Arc::new(StartPositions::new()),
        });
        Self { streams, feed, app }
    }
}

/// Read the body until `wanted` bytes have come.
async fn read(body: &mut Body, wanted: usize) -> Vec<u8> {
    let mut got = Vec::new();
    while got.len() < wanted {
        let frame = tokio::time::timeout(Duration::from_secs(120), body.frame())
            .await
            .expect("the data came")
            .expect("the body goes on")
            .unwrap();
        if let Ok(data) = frame.into_data() {
            got.extend_from_slice(&data);
        }
    }
    got
}

#[tokio::test]
async fn what_is_not_served_is_not_found_as_in_python() {
    let golden = golden();
    let streams = Streams {
        mp3: Broadcaster::new(Config::mp3()),
        flac: None,
        ogg: None,
    };
    let app = router(Api {
        radio: radio(),
        backends: None,
        streams,
        spectrum: None,
        starts: Arc::new(StartPositions::new()),
    });
    for absent in golden["absent"].as_array().unwrap() {
        let path = absent["path"].as_str().unwrap();
        // Without a radio the Python server has no API at all; here the radio is always there.
        if path == "/api/queue" {
            continue;
        }
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            u64::from(response.status().as_u16()),
            absent["status"].as_u64().unwrap(),
            "{path}"
        );
        let text = String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        if path != "/" {
            assert_eq!(text, absent["text"].as_str().unwrap(), "{path}");
        }
    }
}

/// A page that names itself in the stream is waited for by the spectrum, up to five seconds; a quiet connection is
/// asked after fifteen.
#[tokio::test(start_paused = true)]
async fn the_spectrum_waits_for_the_page_and_keeps_the_connection_alive() {
    let world = World::new();
    world.feed.publish(frames_of(0, 10));

    // a page that never connects its stream: five seconds, then "position": null
    let response = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/spectrum?l=ghost")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut body = response.into_body();
    let started = tokio::time::Instant::now();
    let first = read(&mut body, 1).await;
    assert_eq!(
        String::from_utf8(first).unwrap(),
        "event: start\ndata: {\"position\": null}\n\n"
    );
    assert_eq!(started.elapsed(), Duration::from_secs(5));

    // then nothing for fifteen seconds: a comment to show the connection is alive
    let started = tokio::time::Instant::now();
    let beat = read(&mut body, 1).await;
    assert_eq!(String::from_utf8(beat).unwrap(), ": keepalive\n\n");
    assert_eq!(started.elapsed(), Duration::from_secs(15));

    // a page whose stream connected is not waited for: it gets the frames from half a second before its start
    world.streams.mp3.set_fed(0.0);
    for at in (0..60000).step_by(4096) {
        world.streams.mp3.set_fed(at as f64 / 40000.0);
        world.streams.mp3.publish(Bytes::from(vec![0u8; 4096]));
    }
    let stream = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/stream?l=page")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let _stream_body = stream.into_body();
    let response = world
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/spectrum?l=page")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut body = response.into_body();
    let started = tokio::time::Instant::now();
    let first = read(&mut body, 1).await;
    assert!(
        String::from_utf8(first)
            .unwrap()
            .starts_with("event: start\ndata: {\"position\": ")
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}
