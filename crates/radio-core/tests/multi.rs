//! The multi-backend asks for tracks so that each backend has about as many in the pool, whatever the size of its batches.

use radio_core::{
    backend::{
        MusicBackend,
        fake::{FakeBackend, FakeOptions, FakeTrack},
        multi::{BackendSpec, MultiBackend},
    },
    clock::ManualClock,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

fn backend(prefix: &str, batch_size: usize) -> Arc<dyn MusicBackend> {
    let tracks = (0..60)
        .map(|i| FakeTrack {
            track: serde_json::from_value(serde_json::json!({
                "track_id": format!("{prefix}{i}"), "title": "t", "artists": ["a"], "codec": "mp3",
            }))
            .unwrap(),
            file: PathBuf::from("/nonexistent"),
        })
        .collect();
    Arc::new(FakeBackend::new(
        tracks,
        FakeOptions {
            batch_size,
            ..FakeOptions::default()
        },
    ))
}

#[tokio::test]
async fn backends_give_about_as_many_tracks_whatever_their_batches() {
    let specs = vec![
        BackendSpec::new("big", "Big", || backend("b", 12)),
        BackendSpec::new("small", "Small", || backend("s", 3)),
    ];
    let multi = MultiBackend::new(
        specs,
        &["big".to_owned(), "small".to_owned()],
        false,
        HashMap::new(),
        None,
        Arc::new(ManualClock::new(0.0)),
    );
    let mut count: HashMap<String, usize> = HashMap::new();
    for _ in 0..12 {
        for track in multi.station_batch("", &HashSet::new()).await.unwrap() {
            *count.entry(track.source).or_default() += 1;
        }
    }
    let (big, small) = (count["big"], count["small"]);
    // By turns (one batch each) the big one would give four times as many.
    assert!(
        big.abs_diff(small) <= 12,
        "tracks given: big {big}, small {small}"
    );
    assert!(small >= 20, "the small one is asked more often: {small}");
}
