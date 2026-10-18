//! The plans of a blend between two decks against the answers of the original Python radio kept in
//! `tests/golden/radio/pipeline.json`. Task 3.2.1 of docs/rust-port-radio.md.

use radio_core::{
    mix::TrackAnalysis,
    pipeline::{BlendPlan, Deck, beat_period, followed_until, pieces, plan_blend, plan_early},
};
use serde::Deserialize;
use std::{fs, path::PathBuf, sync::Arc};

#[derive(Deserialize)]
struct DeckCase {
    analysis: TrackAnalysis,
    frames: usize,
    beat_matched: bool,
    speed: f64,
}
#[derive(Deserialize)]
struct Plan {
    start: i64,
    following_start: i64,
    length: i64,
    speed: f64,
}
#[derive(Deserialize)]
struct Case {
    current: DeckCase,
    following: DeckCase,
    position: i64,
    plan: Plan,
    matched_after: bool,
    early: Option<Plan>,
    grid: Option<Vec<f64>>,
    beat_period: f64,
    followed_until: i64,
}
#[derive(Deserialize)]
struct Pieces {
    frames: usize,
    sizes: Vec<usize>,
}
#[derive(Deserialize)]
struct Golden {
    cases: Vec<Case>,
    pieces: Vec<Pieces>,
}

fn golden() -> Golden {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/radio/pipeline.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// A deck of that many frames of silence.
fn deck(case: &DeckCase) -> Deck {
    Deck {
        title: "t".into(),
        pcm: Arc::from(vec![0.0f32; case.frames * 2]),
        analysis: case.analysis.clone(),
        beat_matched: case.beat_matched,
        track: None,
        mood: String::new(),
        speed: case.speed,
        generation: 0,
        source: None,
    }
}

fn same(got: BlendPlan, want: &Plan, what: &str) {
    assert_eq!(
        (got.start, got.following_start, got.length),
        (want.start, want.following_start, want.length),
        "{what}"
    );
    assert_eq!(got.speed, want.speed, "{what}: speed");
}

#[test]
fn the_plans_are_where_python_makes_them() {
    let golden = golden();
    let (mut early, mut refused, mut empty, mut unmatched) = (0, 0, 0, 0);
    for (n, case) in golden.cases.iter().enumerate() {
        let what = format!("case {n}");
        let current = deck(&case.current);
        let mut following = deck(&case.following);

        // The early plan is asked for first, on a deck as it was given (the plan below changes `beat_matched`).
        let got_early = plan_early(&current, &following, case.position);
        match (&got_early, &case.early) {
            (Some(got), Some(want)) => same(*got, want, &format!("{what}: early")),
            (None, None) => refused += 1,
            (got, want) => panic!(
                "{what}: early {got:?} against {:?}",
                want.as_ref().map(|w| (w.start, w.length))
            ),
        }
        early += usize::from(got_early.is_some());

        let planned = plan_blend(&current, &mut following, case.position);
        same(planned, &case.plan, &what);
        assert_eq!(
            following.beat_matched, case.matched_after,
            "{what}: beat matched after"
        );
        empty += usize::from(planned.length == 0);
        unmatched += usize::from(case.following.beat_matched && !following.beat_matched);

        assert_eq!(
            beat_period(&current, &following, case.grid.as_deref()),
            case.beat_period,
            "{what}: beat period"
        );
        assert_eq!(
            followed_until(&planned, &following),
            case.followed_until,
            "{what}: followed until"
        );
    }
    // The cases must be of every kind, or the test says little.
    assert!(
        early > 100 && refused > 5 && empty > 0 && unmatched > 0,
        "early {early}, refused {refused}, empty {empty}, unmatched {unmatched}"
    );
}

#[test]
fn the_mix_is_cut_in_pieces_of_a_quarter_of_a_second() {
    for case in golden().pieces {
        let title: Arc<str> = "x".into();
        let pcm = vec![0.0f32; case.frames * 2];
        let sizes: Vec<usize> = pieces(&pcm, &title).map(|p| p.pcm.len() / 2).collect();
        assert_eq!(sizes, case.sizes, "{} frames", case.frames);
    }
}
