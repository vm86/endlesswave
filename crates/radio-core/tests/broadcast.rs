//! The Broadcaster and the ICY titles, with the answers of the original Python radio kept in
//! `tests/golden/stream/broadcast.json`. Task 3.3 of docs/rust-port.md.

use bytes::Bytes;
use radio_core::stream::{
    broadcaster::{Broadcaster, Config},
    icy::{ICY_METAINT, IcyMux, icy_block},
};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc, time::Duration};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/stream")
}

fn golden() -> Value {
    serde_json::from_str(&fs::read_to_string(dir().join("broadcast.json")).unwrap()).unwrap()
}

fn u(value: &Value) -> usize {
    value.as_u64().expect("a number") as usize
}

fn sha1(data: &[u8]) -> String {
    Sha1::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[tokio::test]
async fn a_listener_that_cannot_keep_up_loses_its_oldest_audio() {
    let want = &golden()["overflow"];
    let hub = Broadcaster::new(Config::mp3());
    hub.set_header(b"HEADER".to_vec());
    let listener = hub.subscribe();
    assert_eq!(
        listener.try_recv().unwrap(),
        want["header_first"].as_str().unwrap().as_bytes()
    );
    for i in 0..300usize {
        hub.publish(Bytes::from(vec![(i % 256) as u8; i % 5 + 1]));
    }
    let queue: Vec<Bytes> = std::iter::from_fn(|| listener.try_recv()).collect();
    assert_eq!(queue.len(), u(&want["length"]));
    let hex = |b: &Bytes| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    assert_eq!(hex(&queue[0]), want["first"].as_str().unwrap());
    assert_eq!(hex(queue.last().unwrap()), want["last"].as_str().unwrap());
}

#[test]
fn icy_blocks_and_the_stream_with_titles_are_the_pythons() {
    let icy = &golden()["icy"];
    for (title, block) in icy["titles"]
        .as_array()
        .unwrap()
        .iter()
        .zip(icy["blocks"].as_array().unwrap())
    {
        let hex: String = icy_block(title.as_str().unwrap())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            hex,
            block.as_str().unwrap(),
            "title {:?}",
            title.as_str().unwrap().chars().take(20).collect::<String>()
        );
    }
    assert_eq!(u(&icy["first_block_at"]), ICY_METAINT);

    let changes: BTreeMap<usize, &str> = icy["change_at"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.parse().unwrap(), v.as_str().unwrap()))
        .collect();
    let (mut mux, mut title, mut out) = (IcyMux::default(), "", Vec::new());
    for (i, size) in icy["audio_sizes"].as_array().unwrap().iter().enumerate() {
        title = changes.get(&i).copied().unwrap_or(title);
        let chunk: Vec<u8> = (0..u(size)).map(|k| ((i * 7 + k) % 251) as u8).collect();
        out.extend(mux.push(&chunk, title));
    }
    assert_eq!(out.len(), u(&icy["output_length"]));
    assert_eq!(sha1(&out), icy["output_sha1"].as_str().unwrap());
}

#[tokio::test]
async fn a_listener_waits_for_the_stream_and_the_mix_waits_for_a_listener() {
    let hub = Broadcaster::new(Config::mp3());
    let other = Broadcaster::new(Config::mp3());

    // nobody listens: the wait does not end
    let waiting = {
        let (hub, other) = (Arc::clone(&hub), Arc::clone(&other));
        tokio::spawn(async move { hub.wait_for_a_listener(&[&other]).await })
    };
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!waiting.is_finished());
    // a listener of the other stream ends it
    let listener = other.subscribe();
    tokio::time::timeout(Duration::from_secs(2), waiting)
        .await
        .expect("woken")
        .unwrap();

    // a listener gets what is published after it joined
    let hub2 = Arc::clone(&hub);
    let late = hub.subscribe();
    let reader = tokio::spawn(async move { late.recv().await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    hub2.publish(Bytes::from_static(b"abc"));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), reader)
            .await
            .expect("got it")
            .unwrap(),
        &b"abc"[..]
    );
    drop(listener);
    assert!(!other.has_listeners());
}

#[tokio::test]
async fn a_listener_who_joined_before_the_header_gets_it_when_it_is_set() {
    use radio_core::stream::broadcaster::{Broadcaster, Config};
    let hub = Broadcaster::new(Config::flac());
    let early = hub.subscribe();
    hub.set_header(b"HEADER".to_vec());
    hub.publish(bytes::Bytes::from_static(b"frame"));
    assert_eq!(&early.recv().await[..], b"HEADER");
    assert_eq!(&early.recv().await[..], b"frame");
    // one who comes later gets it as before, and not twice
    let late = hub.subscribe();
    assert_eq!(&late.recv().await[..], b"HEADER");
}
