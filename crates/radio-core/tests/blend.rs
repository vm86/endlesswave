//! The point where a blend swaps the bass, against the answers of the original Python radio kept in
//! `tests/golden/blend/index.json`. Task 1.7 of docs/rust-port.md.

use radio_core::mix::blend;
use serde::Deserialize;
use std::{fs, path::PathBuf};

#[derive(Deserialize)]
struct SwapPoint {
    frames: usize,
    beat_period: f64,
    grid: Option<Vec<f64>>,
    swap: (f64, f64),
}

#[derive(Deserialize)]
struct Index {
    swaps: Vec<SwapPoint>,
}

fn index() -> Index {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/blend/index.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn the_swap_point_is_where_the_original_radio_put_it() {
    for s in index().swaps {
        let (point, ramp) = blend::swap_point(s.frames, s.beat_period, s.grid.as_deref());
        assert_eq!(
            (point, ramp),
            s.swap,
            "{} frames, period {}, grid {:?}",
            s.frames,
            s.beat_period,
            s.grid
        );
    }
}
