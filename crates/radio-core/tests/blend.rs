//! The blends against the Python radio (`tools/gen_golden.py`, `tests/golden/blend/`). Task 1.7 of
//! docs/rust-port.md.
//!
//! The FFTs of numpy and of rustfft round differently, so the pieces are compared by the signal to noise ratio,
//! not bit by bit. The pieces (`*.f32`) are not in git; a case without them is skipped, unless
//! `GOLDEN_REQUIRED` is set.

use radio_core::mix::blend;
use serde::Deserialize;
use std::{fs, path::PathBuf};

/// The least signal to noise ratio (dB) the Rust blend may have against the Python one. The two float32 FFTs
/// are 135 dB or more apart.
const MIN_SNR_DB: f64 = 120.0;

#[derive(Deserialize)]
struct Case {
    name: String,
    function: String,
    beat_period: Option<f64>,
    swap: Option<(f64, f64)>,
}
#[derive(Deserialize)]
struct SwapPoint {
    frames: usize,
    beat_period: f64,
    grid: Option<Vec<f64>>,
    swap: (f64, f64),
}
#[derive(Deserialize)]
struct Index {
    cases: Vec<Case>,
    swaps: Vec<SwapPoint>,
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/blend")
}

fn index() -> Index {
    serde_json::from_str(&fs::read_to_string(dir().join("index.json")).unwrap()).unwrap()
}

fn read(name: &str, part: &str) -> Option<Vec<f32>> {
    let Ok(bytes) = fs::read(dir().join(format!("{name}.{part}.f32"))) else {
        assert!(std::env::var_os("GOLDEN_REQUIRED").is_none(), "{name}: no pieces, run tools/gen_golden.py");
        eprintln!("{name}: no pieces, skipped");
        return None;
    };
    Some(bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect())
}

fn snr_db(got: &[f32], want: &[f32]) -> f64 {
    assert_eq!(got.len(), want.len());
    let signal: f64 = want.iter().map(|&w| f64::from(w).powi(2)).sum();
    let noise: f64 = got.iter().zip(want).map(|(&g, &w)| f64::from(g - w).powi(2)).sum();
    10.0 * (signal / noise.max(1e-30)).log10()
}

#[test]
fn blends_sound_as_the_python_ones_do() {
    for case in index().cases {
        let (Some(out), Some(inc), Some(want)) =
            (read(&case.name, "out"), read(&case.name, "in"), read(&case.name, "want"))
        else {
            continue;
        };
        let got = match case.function.as_str() {
            "crossfade" => blend::crossfade(&out, &inc),
            "bass_swap" => blend::bass_swap(&out, &inc, case.beat_period.unwrap_or(0.0), case.swap),
            "eq_blend" => blend::eq_blend(&out, &inc, case.beat_period.unwrap_or(0.0), case.swap),
            "filter_sweep" => blend::filter_sweep(&out, &inc),
            other => panic!("{other}"),
        };
        let snr = snr_db(&got, &want);
        println!("{:20} {snr:6.1} dB", case.name);
        assert!(snr > MIN_SNR_DB, "{}: {snr:.1} dB", case.name);
    }
}

#[test]
fn the_swap_point_is_where_python_puts_it() {
    for s in index().swaps {
        let (point, ramp) = blend::swap_point(s.frames, s.beat_period, s.grid.as_deref());
        assert_eq!((point, ramp), s.swap, "{} frames, period {}, grid {:?}", s.frames, s.beat_period, s.grid);
    }
}
