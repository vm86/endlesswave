//! The whole daemon: the radio, the API and the streams over a fake backend, with the real ffmpeg. Task 3.4.4 of
//! docs/rust-port-radio.md. Needs ffmpeg and the library of `tests/golden/radio/library/` (not in git); without them the
//! test is skipped, unless `GOLDEN_REQUIRED` is set.

use radio_core::{
    backend::{
        MusicBackend,
        fake::{FakeBackend, FakeOptions, FakeTrack},
    },
    serve::{ServeOptions, serve_with},
};
use serde_json::Value;
use std::{fs, path::PathBuf, sync::Arc, time::Duration};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/radio")
}

#[tokio::test(flavor = "multi_thread")]
async fn the_daemon_streams_a_mix_and_answers_the_api() {
    let library_dir = dir().join("library");
    let have_ffmpeg = std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok();
    if !library_dir.join("t0.wav").exists() || !have_ffmpeg {
        assert!(
            std::env::var_os("GOLDEN_REQUIRED").is_none(),
            "no library or no ffmpeg"
        );
        eprintln!("no library or no ffmpeg: skipped");
        return;
    }
    let library: Vec<Value> =
        serde_json::from_str(&fs::read_to_string(dir().join("library.json")).unwrap()).unwrap();
    let tracks: Vec<FakeTrack> = library
        .iter()
        .map(|entry| {
            let mut asdict = entry.clone();
            let file = library_dir.join(
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

    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("it stops")
        .unwrap()
        .unwrap();
}
