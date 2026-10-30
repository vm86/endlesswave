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

    // The mix by phrases: on at first, switched off and on again, refused when it is not a flag.
    let mixing_url = format!("http://{address}/api/mixing");
    let mixing: Value = reqwest::get(&mixing_url)
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(mixing["phrase"], true);
    let off: Value = reqwest::Client::new()
        .put(&mixing_url)
        .body(r#"{"phrase": false}"#)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(off["phrase"], false);
    let bad = reqwest::Client::new()
        .put(&mixing_url)
        .body(r#"{"phrase": 1}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
    reqwest::Client::new()
        .put(&mixing_url)
        .body(r#"{"phrase": true}"#)
        .send()
        .await
        .unwrap();

    // The console takes the sound of a track of the queue as raw float32 (stereo, 44.1 kHz); an unknown track is a 404.
    let queue: Value = reqwest::get(format!("http://{address}/api/queue"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let wanted = queue["next"][0]["track_id"]
        .as_str()
        .expect("a track waits")
        .to_owned();
    let pcm = reqwest::get(format!("http://{address}/api/track/{wanted}/pcm"))
        .await
        .unwrap();
    assert_eq!(pcm.status(), 200);
    let bytes = pcm.bytes().await.unwrap();
    assert!(
        bytes.len() > 44_100 * 8 && bytes.len() % 8 == 0,
        "{} bytes",
        bytes.len()
    );
    let unknown = reqwest::get(format!("http://{address}/api/track/nope/pcm"))
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404);

    // The arc of moods: refused when wrong, kept when right.
    let client = reqwest::Client::new();
    let arc_url = format!("http://{address}/api/arc");
    let info: Value = client
        .get(&arc_url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["enabled"], false);
    assert_eq!(
        info["moods"],
        serde_json::json!(["sad", "calm", "fun", "active"])
    );
    let refused = client
        .put(&arc_url)
        .body(r#"{"stages": ["sad", "glad"], "stage_tracks": 3}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 400);
    let set: Value = client
        .put(&arc_url)
        .body(r#"{"enabled": true, "stages": ["sad", "fun", "active"], "stage_tracks": 2}"#)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(set["enabled"], true);
    assert_eq!(set["stages"], serde_json::json!(["sad", "fun", "active"]));
    assert_eq!(set["stage_tracks"], 2);
    let off: Value = client
        .put(&arc_url)
        .body(r#"{"enabled": false}"#)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        (off["enabled"].as_bool(), off["stage_tracks"].as_u64()),
        (Some(false), Some(2)),
        "the plan stays"
    );

    // Morning, day, evening: refused when wrong, kept when right.
    let cycle_url = format!("http://{address}/api/cycle");
    let info: Value = client
        .get(&cycle_url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(info["active"], "off");
    assert_eq!(
        info["morning"]["stages"],
        serde_json::json!([60.0, 75.0, 90.0, 105.0, 120.0])
    );
    for wrong in [
        r#"{"active": "noon"}"#,
        r#"{"morning": {"stages": [60], "stage_tracks": 3, "tolerance": 8}}"#,
        r#"{"evening": {"stages": [100, 60], "stage_tracks": 0, "tolerance": 8}}"#,
        r#"{"day": {"bpm_min": 100, "bpm_max": 105}}"#,
    ] {
        let refused = client.put(&cycle_url).body(wrong).send().await.unwrap();
        assert_eq!(refused.status(), 400, "{wrong}");
    }
    let set: Value = client
        .put(&cycle_url)
        .body(r#"{"active": "evening", "day": {"bpm_min": 90, "bpm_max": 120}}"#)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(set["active"], "evening");
    assert_eq!(set["day"]["bpm_min"], 90.0);
    let target = set["target_bpm"].as_f64().unwrap();
    assert!(
        (80.0..=110.0).contains(&target),
        "the evening goes down from 110 BPM: {target}"
    );
    // In the evening (and the morning) the cycle sets the mood: it cannot be changed by hand.
    assert_eq!(set["locks_mood"], true);
    let locked = client
        .put(format!("http://{address}/api/moods"))
        .body(r#"{"moods": ["calm"]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(locked.status(), 409);
    let locked = client
        .put(&arc_url)
        .body(r#"{"enabled": true}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(locked.status(), 409);

    // The schedule: refused when wrong, kept when right.
    assert_eq!(set["schedule"]["enabled"], false);
    for wrong in [
        r#"{"schedule": {"morning": "7am"}}"#,
        r#"{"schedule": {"morning": "08:00", "day": "08:00"}}"#,
        r#"{"schedule": {"enabled": "yes"}}"#,
    ] {
        let refused = client.put(&cycle_url).body(wrong).send().await.unwrap();
        assert_eq!(refused.status(), 400, "{wrong}");
    }
    let scheduled: Value = client
        .put(&cycle_url)
        .body(r#"{"schedule": {"enabled": true, "morning": "05:00", "day": "10:00", "evening": "19:00"}}"#)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(scheduled["schedule"]["evening"], "19:00");
    // With the schedule on the cycle is the clock's, at once: a choice by hand is refused.
    assert_eq!(scheduled["active"], scheduled["scheduled"]);
    let by_hand = client
        .put(&cycle_url)
        .body(r#"{"active": "off"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(by_hand.status(), 409);
    let _ = client
        .put(&cycle_url)
        .body(r#"{"schedule": {"enabled": false}}"#)
        .send()
        .await
        .unwrap();
    let off: Value = client
        .put(&cycle_url)
        .body(r#"{"active": "off"}"#)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(off["active"], "off");
    assert_eq!(off["day"]["bpm_max"], 120.0, "the plans stay");

    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(30), server)
        .await
        .expect("it stops")
        .unwrap()
        .unwrap();
}
