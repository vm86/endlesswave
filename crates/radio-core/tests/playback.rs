//! A manual-to-automatic handoff keeps the encoder and queue, and resumes at the requested source frame.
use radio_core::{
    backend::{
        Track,
        fake::{FakeBackend, FakeOptions, FakeTrack},
    },
    clock::ManualClock,
    encode::{EncodeError, Options, Streams, encode_loop},
    mix::{CHANNELS, SAMPLE_RATE, effects::NoiseSource},
    pipeline::DeckSource,
    radio::{PlaybackInfo, Radio, RadioConfig},
    stream::{
        broadcaster::{Broadcaster, Config},
        encoder::{Encoder, Format},
    },
    testing::write_music_wav,
};
use std::{
    fs, io,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::oneshot;

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn test_radio(count: u32) -> (Scratch, Arc<Radio>) {
    let dir =
        Scratch(std::env::temp_dir().join(format!("radio-playback-{}", uuid::Uuid::new_v4())));
    fs::create_dir_all(dir.0.join("work")).unwrap();
    let tracks = (0..count).map(|i| {
        let file = dir.0.join(format!("{i}.wav"));
        write_music_wav(&file, 120.0, 6.0, i).unwrap();
        FakeTrack { file, track: serde_json::from_value::<Track>(serde_json::json!({
            "track_id": format!("t{i}"), "title": format!("Track {i}"), "artists": ["Test"], "codec": "wav"
        })).unwrap() }
    }).collect();
    let radio = Radio::new(
        Arc::new(FakeBackend::new(tracks, FakeOptions::default())),
        dir.0.join("work"),
        RadioConfig::default(),
        None,
        Arc::new(ManualClock::new(0.0)),
    );
    radio.fill(Some(count as usize)).await.unwrap();
    (dir, radio)
}

struct NoNoise;
impl NoiseSource for NoNoise {
    fn normal(&mut self, count: usize) -> Vec<f64> {
        vec![0.0; count]
    }
}

#[derive(Default)]
struct Log {
    starts: AtomicUsize,
    writes: Mutex<Vec<Vec<f32>>>,
    expected: Mutex<Vec<f32>>,
    applied: Mutex<Option<oneshot::Receiver<PlaybackInfo>>>,
    from: Mutex<String>,
    to: Mutex<String>,
    consume_prepared: bool,
    expected_next: Mutex<String>,
}

struct Spy {
    radio: Arc<Radio>,
    log: Arc<Log>,
}
impl Encoder for Spy {
    async fn write(&mut self, pcm: &[f32]) -> io::Result<()> {
        let count = {
            let mut writes = self.log.writes.lock().unwrap();
            writes.push(pcm.to_vec());
            writes.len()
        };
        if count == 4 {
            let from = self.radio.now_track().unwrap().track_id;
            let prepared = self.radio.upcoming(1)[0].track_id.clone();
            let to = if self.log.consume_prepared {
                self.radio.pool()[0]
                    .track
                    .as_ref()
                    .unwrap()
                    .track_id
                    .clone()
            } else {
                prepared.clone()
            };
            assert_ne!(from, to);
            let expected = self.radio.track_pcm(&to).await.unwrap();
            let applied = if self.log.consume_prepared {
                *self.log.expected_next.lock().unwrap() = self
                    .radio
                    .pool()
                    .iter()
                    .find_map(|c| {
                        c.track
                            .as_ref()
                            .filter(|t| t.track_id != to)
                            .map(|t| t.track_id.clone())
                    })
                    .unwrap();
                self.radio
                    .resume_moving_playback(&to, 1.25, None, 0.0, &[prepared])
                    .await
                    .unwrap()
            } else {
                self.radio
                    .resume_playback(&to, 1.25, Some(&from))
                    .await
                    .unwrap()
            };
            *self.log.expected.lock().unwrap() = expected;
            *self.log.applied.lock().unwrap() = Some(applied);
            *self.log.from.lock().unwrap() = from;
            *self.log.to.lock().unwrap() = to;
        }
        if count == 6 {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, "test finished"));
        }
        Ok(())
    }
    async fn stop(self) {}
}

async fn run_handoff(consume_prepared: bool) {
    let (_dir, radio) = test_radio(if consume_prepared { 4 } else { 3 }).await;
    let hub = Broadcaster::new(Config::mp3());
    let _listener = hub.subscribe();
    let streams = Streams {
        mp3: hub,
        flac: None,
        ogg: None,
    };
    let log = Arc::new(Log {
        consume_prepared,
        ..Log::default()
    });
    let factory = {
        let (radio, log) = (radio.clone(), log.clone());
        move |_: Format, _: Arc<Broadcaster>| {
            log.starts.fetch_add(1, Ordering::SeqCst);
            Ok(Spy {
                radio: radio.clone(),
                log: log.clone(),
            })
        }
    };
    let result = encode_loop(
        radio.clone(),
        Arc::new(ManualClock::new(0.0)),
        0.0,
        Arc::new(Mutex::new(NoNoise)),
        &streams,
        None,
        &factory,
        Options::default(),
    )
    .await;
    assert!(matches!(result, Err(EncodeError::Encoder(_))));
    assert_eq!(log.starts.load(Ordering::SeqCst), 1);
    let applied = log.applied.lock().unwrap().take().unwrap();
    let info = applied.await.unwrap();
    assert_eq!(info.track.track_id, *log.to.lock().unwrap());
    assert!((info.position - 1.25).abs() < 1.0 / f64::from(SAMPLE_RATE));
    {
        let expected = log.expected.lock().unwrap();
        let writes = log.writes.lock().unwrap();
        let start = (1.25 * f64::from(SAMPLE_RATE)) as usize * CHANNELS;
        assert_eq!(&writes[4], &expected[start..start + writes[4].len()]);
    }
    let heard = radio.playback(Some(info.mix_position + 0.1)).unwrap();
    assert_eq!(heard.track.track_id, info.track.track_id);
    assert!((heard.position - 1.35).abs() < 1e-6);
    let queue = radio.queue(10, 10);
    assert_eq!(queue["now"]["track_id"], info.track.track_id);
    let next = if consume_prepared {
        log.expected_next.lock().unwrap().clone()
    } else {
        log.from.lock().unwrap().clone()
    };
    assert_eq!(queue["next"][0]["track_id"], next);
    assert!(!radio.playback_requested());
    assert!(
        radio
            .resume_playback(&info.track.track_id, f64::NAN, None)
            .await
            .is_err()
    );
    assert!(
        radio
            .resume_playback(&info.track.track_id, -1.0, None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_handoff_resumes_the_source_position_without_restarting_the_encoder() {
    run_handoff(false).await;
}

#[tokio::test]
async fn tracks_heard_in_dj_are_not_put_back_in_the_queue() {
    run_handoff(true).await;
}

#[tokio::test]
async fn deck_leases_survive_queue_removal_and_out_of_order_loads() {
    let (_dir, radio) = test_radio(5).await;
    let a = radio.leased_track_pcm("t0", "console", 0, 1).await.unwrap();
    for version in 1..=3 {
        radio
            .leased_track_pcm(&format!("t{version}"), "console", 1, version)
            .await
            .unwrap();
    }
    let b = radio.track_pcm("t3").await.unwrap();
    radio.leased_track_pcm("t1", "console", 1, 1).await.unwrap();
    radio.track_pcm("t4").await.unwrap();
    radio.queue_remove("t0").unwrap();
    radio.queue_remove("t3").unwrap();
    assert_eq!(radio.track_pcm("t0").await.unwrap(), a);
    assert_eq!(radio.track_pcm("t3").await.unwrap(), b);
    assert!(radio.leased_track_pcm("t4", "console", 2, 4).await.is_err());
    radio.release_playback_leases("console");
    assert!(radio.track_pcm("t0").await.is_err());
    assert!(radio.track_pcm("t3").await.is_err());
}

#[tokio::test]
async fn an_abandoned_handoff_can_be_retried() {
    let (_dir, radio) = test_radio(3).await;
    let first = radio.resume_playback("t0", 1.0, None).await.unwrap();
    drop(first);
    let next = radio.resume_playback("t1", 2.0, None).await.unwrap();
    assert!(radio.playback_requested());
    radio.cancel_playback_handoff();
    assert!(!radio.playback_requested());
    assert!(next.await.is_err());
    assert!(radio.user_paused());
}

#[tokio::test]
async fn playback_http_metadata_and_deck_leases_match_the_encoded_coordinates() {
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;
    let (_dir, radio) = test_radio(2).await;
    let deck = radio.next_deck(None).await.unwrap();
    radio.started(&deck);
    radio.published(
        radio_core::pipeline::Playhead::solo(&deck, None, (SAMPLE_RATE * 2) as i64),
        10.0,
        0.25,
    );
    let app = radio_core::api::router(radio_core::api::Api {
        radio: radio.clone(),
        backends: None,
        streams: Streams {
            mp3: Broadcaster::new(Config::mp3()),
            flac: None,
            ogg: None,
        },
        spectrum: None,
        starts: Arc::new(radio_core::spectrum::StartPositions::new()),
    });
    let response = app
        .clone()
        .oneshot(
            Request::get("/api/playback?at=10.125")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let info: PlaybackInfo =
        serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(info.track.track_id, deck.track.as_ref().unwrap().track_id);
    assert_eq!(info.position, 2.125);
    assert_eq!(info.mix_position, 10.125);
    for (at, expected) in [("NaN", 400), ("99999", 409)] {
        let response = app
            .clone()
            .oneshot(
                Request::get(format!("/api/playback?at={at}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
    let response = app
        .clone()
        .oneshot(Request::get("/stream").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()["x-stream-position"], "0");
    drop(response);
    let response = app
        .clone()
        .oneshot(
            Request::get(format!(
                "/api/track/{}/pcm?dj_owner=test&dj_slot=0&dj_version=1",
                info.track.track_id
            ))
            .body(Body::empty())
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes()
            .len(),
        deck.pcm.len() * 4
    );
    let response = app
        .oneshot(
            Request::post("/api/playback")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"track_id": info.track.track_id, "position": -1})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert!(!radio.playback_requested());
}
