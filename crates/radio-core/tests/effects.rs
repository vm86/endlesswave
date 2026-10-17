//! The effects, the limiter and `blend` against the Python radio (`tools/gen_golden.py`,
//! `tests/golden/effects/`). Task 1.9 of docs/rust-port.md.
//!
//! The noise of the riser and of the boom is the very noise the Python radio drew (`*.noise.f64`), given back
//! in the same order, so the pieces can be compared by the signal to noise ratio. Pieces are not in git; a case
//! without them is skipped, unless `GOLDEN_REQUIRED` is set.

use radio_core::mix::{
    effects::{self, Effect, NoiseSource},
    plan::Transition,
};
use serde::Deserialize;
use std::{fs, path::PathBuf};

const MIN_SNR_DB: f64 = 120.0;

#[derive(Deserialize)]
struct Case {
    name: String,
    function: String,
    frames: Option<usize>,
    cut: Option<usize>,
    level: Option<f64>,
    drop: Option<i64>,
    period: Option<f64>,
    kind: Option<String>,
    effects: Option<f64>,
    grid: Option<Vec<f64>>,
    draws: Option<Vec<usize>>,
    applied: Option<(String, String)>,
}
#[derive(Deserialize)]
struct Index {
    cases: Vec<Case>,
}

/// Gives back the draws that were recorded, in order, and checks that the same amounts are asked for.
struct Replay {
    draws: std::vec::IntoIter<Vec<f64>>,
}

impl NoiseSource for Replay {
    fn normal(&mut self, count: usize) -> Vec<f64> {
        let draw = self.draws.next().expect("more noise asked for than Python drew");
        assert_eq!(draw.len(), count, "the amount of noise differs from Python's");
        draw
    }
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/effects")
}

fn read(name: &str, part: &str) -> Option<Vec<f32>> {
    let Ok(bytes) = fs::read(dir().join(format!("{name}.{part}.f32"))) else {
        assert!(std::env::var_os("GOLDEN_REQUIRED").is_none(), "{name}: no pieces, run tools/gen_golden.py");
        eprintln!("{name}: no pieces, skipped");
        return None;
    };
    Some(bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect())
}

fn replay(name: &str, draws: &[usize]) -> Replay {
    let bytes = fs::read(dir().join(format!("{name}.noise.f64"))).expect("the noise file");
    let flat: Vec<f64> = bytes.chunks_exact(8).map(|b| f64::from_le_bytes(b.try_into().unwrap())).collect();
    let mut parts = Vec::new();
    let mut rest = &flat[..];
    for &n in draws {
        let (head, tail) = rest.split_at(n);
        parts.push(head.to_vec());
        rest = tail;
    }
    assert!(rest.is_empty());
    Replay { draws: parts.into_iter() }
}

fn snr_db(got: &[f32], want: &[f32]) -> f64 {
    assert_eq!(got.len(), want.len(), "lengths");
    let signal: f64 = want.iter().map(|&w| f64::from(w).powi(2)).sum();
    let noise: f64 = got.iter().zip(want).map(|(&g, &w)| f64::from(g - w).powi(2)).sum();
    10.0 * (signal / noise.max(1e-30)).log10()
}

fn kind(name: &str) -> Transition {
    match name {
        "crossfade" => Transition::Crossfade,
        "bass swap" => Transition::BassSwap,
        "eq blend" => Transition::EqBlend,
        "filter sweep" => Transition::FilterSweep,
        other => panic!("{other}"),
    }
}

#[test]
fn the_effects_the_limiter_and_the_blend_are_the_pythons() {
    let path = dir().join("index.json");
    let index: Index = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let mut compared = 0;
    for case in index.cases {
        let name = case.name.as_str();
        let Some(want) = read(name, "want") else { continue };
        let draws = case.draws.clone().unwrap_or_default();
        let got = match case.function.as_str() {
            "riser" => effects::riser(case.frames.unwrap(), &mut replay(name, &draws)),
            "impact" => effects::impact(case.frames.unwrap(), &mut replay(name, &draws)),
            "echo_out" => effects::echo_out(&read(name, "in").unwrap(), case.cut.unwrap(), case.level.unwrap()),
            "limit" => effects::limit(&read(name, "in").unwrap(), effects::LIMIT_CEILING),
            "add_drop" => effects::add_drop(
                &read(name, "in").unwrap(),
                case.drop.unwrap(),
                case.period.unwrap(),
                case.level.unwrap(),
                &mut replay(name, &draws),
            ),
            "blend" => {
                let (out, inc) = (read(name, "out").unwrap(), read(name, "in").unwrap());
                let (frames, period, level) = (out.len() / 2, case.period.unwrap(), case.effects.unwrap());
                let (applied_kind, applied_effect) = case.applied.clone().unwrap();
                let applied = effects::applied_blend(kind(case.kind.as_deref().unwrap()), frames, period, level);
                assert_eq!(applied.kind, kind(&applied_kind), "{name}: kind");
                assert_eq!(applied.effect.name(), applied_effect, "{name}: effect");
                if applied.effect == Effect::None {
                    assert!(draws.is_empty());
                }
                effects::blend(
                    kind(case.kind.as_deref().unwrap()),
                    &out,
                    &inc,
                    period,
                    level,
                    &mut replay(name, &draws),
                    case.grid.as_deref(),
                )
            }
            other => panic!("{other}"),
        };
        let snr = snr_db(&got, &want);
        println!("{name:18} {snr:6.1} dB");
        assert!(snr > MIN_SNR_DB, "{name}: {snr:.1} dB");
        compared += 1;
    }
    assert!(compared == 0 || compared == 19, "{compared} compared");
}

#[test]
fn a_limiter_leaves_a_quiet_piece_alone_and_keeps_a_loud_one_under_the_ceiling() {
    let quiet = vec![0.5f32, -0.5, 0.25, 0.1];
    assert_eq!(effects::limit(&quiet, 0.97), quiet);
    assert!(effects::limit(&[], 0.97).is_empty());
    let loud: Vec<f32> = (0..20000).map(|i| (i as f32 * 0.01).sin() * 3.0).collect();
    let limited = effects::limit(&loud, 0.97);
    assert_eq!(limited.len(), loud.len());
    assert!(limited.iter().all(|x| x.abs() <= 1.0));
}
