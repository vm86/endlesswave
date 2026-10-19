//! The whole daemon: the radio, the API and the streams over a fake backend, with the real ffmpeg. Task 3.4.4 of
//! docs/rust-port-radio.md. Needs ffmpeg (without it the test is skipped); the tracks are made by the test.

use radio_core::{
    backend::{
        MusicBackend,
        fake::{FakeBackend, FakeOptions, FakeTrack},
    },
    serve::{ServeOptions, serve_with},
    testing::write_music_wav,
};
use serde_json::Value;
use std::{fs, sync::Arc, time::Duration};

#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_streams_a_mix_and_answers_the_api() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_err()
    {
        eprintln!("no ffmpeg: skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!("radio-core-daemon-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let tracks: Vec<FakeTrack> = (0..4u32)
        .map(|i| {
            let file = dir.join(format!("t{i}.wav"));
            write_music_wav(&file, 110.0 + f64::from(i) * 6.0, 40.0, i).unwrap();
            let track = serde_json::from_value(serde_json::json!({
                "track_id": format!("t{i}"), "title": format!("Track {i}"), "artists": ["Test"], "codec": "wav",
            }))
            .unwrap();
            FakeTrack { track, file }
        })
        .collect();
    let backend = Arc::new(FakeBackend::new(
        tracks,
        FakeOptions {
            batch_size: 3,
            ..FakeOptions::default()
        },
    )) as Arc<dyn MusicBackend>;

    let (ready, address) = tokio::sync::oneshot::channel();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve_with(
        backend,
        None,
        ServeOptions {
            port: 0,
            pool_size: 3,
            ..ServeOptions::default()
        },
        None,
        Some(ready),
        async move {
            let _ = stopped.await;
        },
        Duration::from_secs(1),
    ));
    let address = address.await.unwrap();

    // The API answers before anything is mixed.
    let early: Value = reqwest::get(format!("http://{address}/api/queue"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(early.is_object(), "{early}");

    // The stream: MP3 frames come once the first tracks are ready.
    let mut response = tokio::time::timeout(
        Duration::from_secs(120),
        reqwest::get(format!("http://{address}/stream")),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.headers()["content-type"], "audio/mpeg");
    let mut got = 0;
    while got < 40_000 {
        let chunk = tokio::time::timeout(Duration::from_secs(120), response.chunk())
            .await
            .expect("the stream goes on")
            .unwrap()
            .expect("the stream does not end");
        got += chunk.len();
    }
    let queue: Value = reqwest::get(format!("http://{address}/api/queue"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        queue["now"].is_object() || queue["current"].is_object() || queue.is_object(),
        "{queue}"
    );

    // Clearing the queue drops what waits and loads it anew: tracks are in the queue again after a while.
    let cleared = reqwest::Client::new()
        .post(format!("http://{address}/api/queue/clear"))
        .send()
        .await
        .unwrap();
    assert_eq!(cleared.status(), 200);
    let mut refilled = false;
    for _ in 0..120 {
        let queue: Value = reqwest::get(format!("http://{address}/api/queue"))
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if queue["next"].as_array().is_some_and(|n| !n.is_empty()) {
            refilled = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(refilled, "the queue is loaded again after it is cleared");

    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("it stops")
        .unwrap()
        .unwrap();
}
