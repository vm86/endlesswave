//! The answers of the Python radio (`tools/gen_golden.py`) against this crate.
//!
//! The signals (`*.f32`) are not in git: `tools/gen_golden.py` makes them. A case without its signal is
//! skipped with a note; set `GOLDEN_REQUIRED=1` (CI does) to make it an error.

use radio_core::mix::{self, TrackAnalysis};
use serde::Deserialize;
use std::{fs, path::PathBuf};

#[derive(Debug, Deserialize)]
struct Golden {
    sample_rate: u32,
    frames: usize,
    key_detector: String,
    loudness_meter: String,
    analysis: TrackAnalysis,
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

fn cases() -> Vec<(String, Golden)> {
    let mut found: Vec<_> = fs::read_dir(dir())
        .unwrap()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            if path.extension()? != "json" {
                return None;
            }
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let golden = serde_json::from_str(&fs::read_to_string(&path).unwrap())
                .unwrap_or_else(|err| panic!("{name}.json: {err}"));
            Some((name, golden))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// The signal of a case as stereo, interleaved; `None` when its file was not made.
fn signal(name: &str, golden: &Golden) -> Option<Vec<f32>> {
    let path = dir().join(format!("{name}.f32"));
    let Ok(bytes) = fs::read(&path) else {
        assert!(
            std::env::var_os("GOLDEN_REQUIRED").is_none(),
            "{name}: no signal, run tools/gen_golden.py"
        );
        eprintln!("{name}: no signal ({}), skipped", path.display());
        return None;
    };
    let mono: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
    assert_eq!(mono.len(), golden.frames, "{name}: the signal and its json disagree");
    Some(mono.iter().flat_map(|&s| [s, s]).collect())
}

#[test]
fn the_golden_files_are_there_and_readable() {
    let cases = cases();
    assert!(cases.len() >= 10, "only {} cases", cases.len());
    for (name, golden) in &cases {
        assert_eq!(golden.sample_rate, mix::SAMPLE_RATE, "{name}");
        assert!(golden.frames > 0, "{name}");
        // The key is found by Essentia: the key of these files must be too, or the numbers are not the ones to match.
        assert_eq!(golden.key_detector, "essentia", "{name}");
        assert_eq!(golden.loudness_meter, "ebu r128", "{name}");
        let a = &golden.analysis;
        assert!(a.key >= -1 && a.key < 24, "{name}");
        assert!(a.start >= 0 && a.end as usize <= golden.frames, "{name}");
    }
}

#[test]
fn the_signals_match_their_files() {
    for (name, golden) in cases() {
        if let Some(pcm) = signal(&name, &golden) {
            assert_eq!(pcm.len(), golden.frames * mix::CHANNELS, "{name}");
        }
    }
}

/// Everything but the key: 1.3 the tempo, the beats and the sections, 1.4 the loudness.
#[test]
fn analysis_matches_python() {
    for (name, golden) in cases() {
        let Some(pcm) = signal(&name, &golden) else { continue };
        let got = mix::analyze(&pcm);
        let want = &golden.analysis;
        let note = format!("{name}: got {got:?}, python {want:?}");
        assert!((got.bpm - want.bpm).abs() < 0.01, "bpm, {note}");
        // A beat is found to half a frame of the flux (a quarter of a hop); the same grid is meant.
        assert!((got.head_beat - want.head_beat).abs() < 1.0, "head beat, {note}");
        assert!((got.tail_beat - want.tail_beat).abs() < 1.0, "tail beat, {note}");
        assert_eq!((got.start, got.end), (want.start, want.end), "audible range, {note}");
        assert_eq!((got.intro_end, got.outro_start), (want.intro_end, want.outro_start), "sections, {note}");
        // ebur128 and Essentia differ below ~100 Hz (Essentia presses the bass harder: 0.2 dB on a 60 Hz kick, 1.3 dB
        // on a 30 Hz sine); the same numbers come with Essentia's own meter (task 1.5.2), then this is 0.01.
        assert!((got.loudness_db - want.loudness_db).abs() < 0.25, "loudness, {note}");
    }
}

/// Task 1.5 of docs/rust-port.md.
#[test]
#[ignore = "the key is not found yet"]
fn key_matches_python() {
    for (name, golden) in cases() {
        let Some(pcm) = signal(&name, &golden) else { continue };
        assert_eq!(mix::analyze(&pcm).key, golden.analysis.key, "{name}");
    }
}
