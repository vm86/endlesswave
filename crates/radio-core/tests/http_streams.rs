//! What a listener of the streams and of the spectrum receives from the server, against the Python radio's
//! (`tools/gen_golden.py`, `tests/golden/stream/http_streams.json`). Task 4.4 of docs/rust-port.md.
//!
//! The server of Python is run by aiohttp and asked for the streams (made by the radio's own ffmpeg commands, as in the
//! tests of the Broadcaster); here the router is asked without a socket. The headers, the number of bytes and their sha1
//! must be the same. The streams (`*.bin`) are not in git; without them the tests are skipped, unless `GOLDEN_REQUIRED`
//! is set.

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
    stream::{
        broadcaster::{Broadcaster, Config},
        frames,
    },
};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::{collections::HashMap, fs, path::PathBuf, sync::Arc, time::Duration};
use tower::ServiceExt as _;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/stream")
}

fn golden() -> Value {
    serde_json::from_str(&fs::read_to_string(dir().join("http_streams.json")).unwrap()).unwrap()
}

fn sha1(data: &[u8]) -> String {
    Sha1::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn u(value: &Value) -> usize {
    value.as_u64().unwrap() as usize
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
    data: HashMap<&'static str, Vec<u8>>,
    streams: Streams,
    feed: Arc<SpectrumFeed>,
    app: Router,
}

impl World {
    fn new(data: HashMap<&'static str, Vec<u8>>) -> Self {
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
        Self {
            data,
            streams,
            feed,
            app,
        }
    }

    fn hub(&self, name: &str) -> &Arc<Broadcaster> {
        match name {
            "mp3" => &self.streams.mp3,
            "flac" => self.streams.flac.as_ref().unwrap(),
            _ => self.streams.ogg.as_ref().unwrap(),
        }
    }

    /// `publish` of the script: chunks of the stream, with the position of the mix going on by a step for each.
    fn publish(&self, op: &Value) {
        let name = op["stream"].as_str().unwrap();
        let (hub, data) = (self.hub(name), &self.data[name]);
        let mut start = u(&op["from"]);
        if start == 0 && name != "mp3" {
            let length = if name == "flac" {
                frames::flac_header_length(data)
            } else {
                frames::ogg_header_length(data)
            }
            .unwrap()
            .unwrap();
            hub.set_header(data[..length].to_vec());
            start = length;
        }
        let (to, chunk) = (u(&op["to"]), u(&op["chunk"]));
        let mut fed = op["fed"].as_f64().unwrap_or(0.0);
        for at in (start..to).step_by(chunk) {
            fed += op["fed_step"].as_f64().unwrap();
            hub.set_fed(fed);
            hub.publish(Bytes::copy_from_slice(&data[at..(at + chunk).min(to)]));
        }
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
async fn the_listener_of_a_stream_gets_what_python_serves() {
    let mut data = HashMap::new();
    for name in ["mp3", "flac", "ogg"] {
        match fs::read(dir().join(format!("{name}.bin"))) {
            Ok(bytes) => data.insert(name, bytes),
            Err(_) => {
                assert!(
                    std::env::var_os("GOLDEN_REQUIRED").is_none(),
                    "{name}: run tools/gen_golden.py"
                );
                eprintln!("{name}: no stream, skipped");
                return;
            }
        };
    }
    for scenario in golden()["scenarios"].as_array().unwrap() {
        let name = scenario["name"].as_str().unwrap();
        let world = World::new(data.clone());
        world
            .streams
            .mp3
            .set_title(scenario["title"].as_str().unwrap_or(""));
        for op in scenario["setup"].as_array().unwrap() {
            world.publish(op);
        }
        if let Some(count) = scenario["spectrum"].as_u64() {
            for first in (0..count as usize).step_by(10) {
                world.feed.publish(frames_of(first, 10));
            }
        }
        for call in scenario["results"].as_array().unwrap() {
            let what = format!("{name}: {}", call["path"].as_str().unwrap());
            let mut request = Request::builder().uri(call["path"].as_str().unwrap());
            for (key, value) in call["headers"].as_object().into_iter().flatten() {
                request = request.header(key, value.as_str().unwrap());
            }
            let response = world
                .app
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                u64::from(response.status().as_u16()),
                call["status"].as_u64().unwrap(),
                "{what}: status"
            );
            // the headers that Python sent, with its values, and no others of the kind
            let wanted = call["response_headers"].as_object().unwrap();
            for (key, want) in wanted {
                let got = response.headers().get(key).and_then(|v| v.to_str().ok());
                assert_eq!(got, want.as_str(), "{what}: the header {key}");
            }
            for key in [
                "content-type",
                "cache-control",
                "icy-name",
                "icy-metaint",
                "access-control-allow-origin",
                "x-accel-buffering",
            ] {
                assert_eq!(
                    response.headers().contains_key(key),
                    wanted.contains_key(key),
                    "{what}: the header {key} is there or not"
                );
            }
            let mut body = response.into_body();
            let reads = call["reads"].as_array().unwrap();
            let mut steps: Vec<Option<&Value>> = vec![None];
            steps.extend(call["then"].as_array().into_iter().flatten().map(Some));
            if call["then_frames"].is_array() {
                steps.push(Some(&call["then_frames"]));
            }
            for (n, (step, read_result)) in steps.iter().zip(reads).enumerate() {
                match step {
                    Some(op) if op.is_array() => {
                        world.feed.publish(frames_of(u(&op[0]), u(&op[1])))
                    }
                    Some(op) => world.publish(op),
                    None => {}
                }
                let want_bytes = u(&read_result["bytes"]);
                let got = read(&mut body, want_bytes).await;
                assert_eq!(got.len(), want_bytes, "{what}: read {n}: bytes");
                assert_eq!(
                    sha1(&got),
                    read_result["sha1"].as_str().unwrap(),
                    "{what}: read {n}: sha1"
                );
                if let Some(text) = read_result["text"].as_str() {
                    assert_eq!(
                        String::from_utf8(got).unwrap(),
                        text,
                        "{what}: read {n}: the events"
                    );
                }
            }
        }
        println!("{name}: the same");
    }
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
    let world = World::new(HashMap::new());
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
