//! Normalization in the real deck loader, without a server or an external music service.

use radio_core::{
    backend::{
        Track,
        fake::{FakeBackend, FakeOptions, FakeTrack},
    },
    clock::ManualClock,
    mix::{decode, effects::LIMIT_CEILING, loudness},
    radio::{Radio, RadioConfig},
    testing::write_music_wav,
};
use std::{fs, path::PathBuf, sync::Arc};

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn decks_are_levelled_without_changing_the_source_analysis_or_files() {
    let dir = Scratch(std::env::temp_dir().join(format!("deck-loudness-{}", uuid::Uuid::new_v4())));
    fs::create_dir_all(dir.0.join("work")).unwrap();
    let loud = dir.0.join("loud.wav");
    let quiet = dir.0.join("quiet.wav");
    write_music_wav(&loud, 120.0, 6.0, 0).unwrap();
    let loud_bytes = fs::read(&loud).unwrap();
    let mut quiet_bytes = loud_bytes.clone();
    for sample in quiet_bytes[44..].chunks_exact_mut(2) {
        let value = i16::from_le_bytes(sample.try_into().unwrap()) / 2;
        sample.copy_from_slice(&value.to_le_bytes());
    }
    fs::write(&quiet, &quiet_bytes).unwrap();
    let tracks = [("loud", loud.clone()), ("quiet", quiet.clone())]
        .into_iter()
        .map(|(id, file)| FakeTrack {
            track: serde_json::from_value::<Track>(serde_json::json!({
                "track_id": id, "title": id, "artists": ["Test"], "codec": "wav"
            }))
            .unwrap(),
            file,
        })
        .collect();
    let backend = Arc::new(FakeBackend::new(
        tracks,
        FakeOptions {
            batch_size: 2,
            ..FakeOptions::default()
        },
    ));
    let radio = Radio::new(
        backend,
        dir.0.join("work"),
        RadioConfig::default(),
        None,
        Arc::new(ManualClock::new(0.0)),
    );
    radio.fill(Some(2)).await.unwrap();
    let mut source_levels = Vec::new();
    let mut played_levels = Vec::new();
    for _ in 0..2 {
        let deck = radio.next_deck(None).await.unwrap();
        let source = deck.source.as_ref().unwrap();
        assert_eq!(deck.analysis, source.analysis);
        let decoded = decode::decode_file(&source.path).unwrap();
        let source_loudness = loudness::measure_loudness(&decoded);
        assert!((source_loudness - deck.analysis.loudness_db).abs() < 0.1);
        let played_loudness = loudness::measure_loudness(&deck.pcm);
        assert!(
            (played_loudness - loudness::TARGET_LUFS).abs() < 0.1,
            "{played_loudness}"
        );
        assert!((played_loudness - source_loudness - deck.gain_db).abs() < 0.1);
        assert!(
            deck.pcm
                .iter()
                .all(|x| x.is_finite() && x.abs() <= LIMIT_CEILING)
        );
        source_levels.push(source_loudness);
        played_levels.push(played_loudness);
    }
    assert!((source_levels[0] - source_levels[1]).abs() > 5.9);
    assert!((played_levels[0] - played_levels[1]).abs() < 0.1);
    assert_eq!(fs::read(loud).unwrap(), loud_bytes);
    assert_eq!(fs::read(quiet).unwrap(), quiet_bytes);
}
