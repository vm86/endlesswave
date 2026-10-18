//! Whole blends between decks of the library against the Python radio's (`tools/gen_golden.py`,
//! `tests/golden/radio/transitions.json`). Tasks 3.2.2 and 3.2.3 of docs/rust-port-radio.md.
//!
//! The tracks are decoded by ffmpeg from `tests/golden/radio/library/*.wav` (not in git), the plan and the analysis are
//! Python's own, and the noise of the effects is the noise Python drew. Without the library, or ffmpeg, the test is
//! skipped, unless `GOLDEN_REQUIRED` is set.

use async_trait::async_trait;
use radio_core::{
    clock::ManualClock,
    mix::{
        TrackAnalysis, decode,
        effects::{Effect, NoiseSource},
        plan::Transition,
    },
    pipeline::{BlendPlan, BlendReport, Deck, DeckError, DeckSource, SharedNoise, transition},
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Deserialize)]
struct TrackCase {
    title: String,
    file: String,
    analysis: TrackAnalysis,
}
#[derive(Deserialize)]
struct Input {
    speed: f64,
}
#[derive(Deserialize)]
struct PlanCase {
    start: i64,
    following_start: i64,
    length: i64,
    speed: f64,
}
#[derive(Deserialize)]
struct Case {
    name: String,
    current: usize,
    following: usize,
    effects: f64,
    input: Input,
    plan: PlanCase,
    matched_after_plan: bool,
    beat_matched_after: bool,
    events: Vec<String>,
    report: Value,
    pieces: Vec<(String, usize)>,
    draws: Vec<usize>,
}
#[derive(Deserialize)]
struct Golden {
    tracks: Vec<TrackCase>,
    cases: Vec<Case>,
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/radio")
}

/// Hands back the noise that was recorded, in order.
struct Replay(std::vec::IntoIter<Vec<f64>>);

impl NoiseSource for Replay {
    fn normal(&mut self, count: usize) -> Vec<f64> {
        let draw = self.0.next().expect("more noise asked for than Python drew");
        assert_eq!(draw.len(), count, "the amount of noise differs from Python's");
        draw
    }
}

/// Notes what the mix tells the radio.
#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<String>>,
    reports: Mutex<Vec<BlendReport>>,
}

#[async_trait]
impl DeckSource for Recorder {
    fn skip_requested(&self) -> bool {
        false
    }
    async fn next_deck(&self, _current: Option<&TrackAnalysis>) -> Result<Deck, DeckError> {
        Err(DeckError("no decks".into()))
    }
    fn is_stale(&self, _deck: &Deck) -> bool {
        false
    }
    fn skip_taken(&self) {}
    fn transition(&self, kind: Transition, current: &Deck, following: &Deck, effect: Effect, report: Option<BlendReport>) {
        self.events.lock().unwrap().push(format!("transition {} {} {} -> {}", kind.name(), effect.name(), current.title, following.title));
        self.reports.lock().unwrap().extend(report);
    }
    fn started(&self, deck: &Deck) {
        self.events.lock().unwrap().push(format!("started {}", deck.title));
    }
    fn finished(&self, deck: &Deck) {
        self.events.lock().unwrap().push(format!("finished {}", deck.title));
    }
    fn user_paused(&self) -> bool {
        false
    }
    fn hold(&self, _held: bool) {}
    async fn wait_resumed(&self) {}
}

fn snr_db(got: &[f32], want: &[f32]) -> f64 {
    assert_eq!(got.len(), want.len(), "lengths");
    let signal: f64 = want.iter().map(|&w| f64::from(w).powi(2)).sum();
    let noise: f64 = got.iter().zip(want).map(|(&g, &w)| f64::from(g - w).powi(2)).sum();
    10.0 * (signal / noise.max(1e-30)).log10()
}

#[tokio::test]
async fn blends_are_made_and_told_as_python_does() {
    let golden: Golden = serde_json::from_str(&fs::read_to_string(dir().join("transitions.json")).unwrap()).unwrap();
    let library = dir().join("library");
    if !library.join("t0.wav").exists() {
        assert!(std::env::var_os("GOLDEN_REQUIRED").is_none(), "no library, run tools/gen_golden.py");
        eprintln!("no library: skipped");
        return;
    }
    // Every track of the library decoded once, as the radio decodes a deck.
    let pcm: Vec<Arc<[f32]>> = golden
        .tracks
        .iter()
        .map(|t| match decode::decode_with_ffmpeg(std::path::Path::new("ffmpeg"), &library.join(&t.file)) {
            Ok(pcm) => Arc::from(pcm),
            Err(error) => {
                assert!(std::env::var_os("GOLDEN_REQUIRED").is_none(), "{error}");
                eprintln!("{error}: skipped");
                Arc::from(Vec::new())
            }
        })
        .collect();
    if pcm.iter().any(|p| p.is_empty()) {
        return;
    }
    let deck = |index: usize, beat_matched: bool, speed: f64| Deck {
        title: golden.tracks[index].title.as_str().into(),
        pcm: pcm[index].clone(),
        analysis: golden.tracks[index].analysis.clone(),
        beat_matched,
        track: None,
        mood: String::new(),
        speed,
        generation: 0,
        source: None,
    };

    let clock = ManualClock::new(0.0);
    let mut kinds = std::collections::BTreeSet::new();
    for case in &golden.cases {
        let name = &case.name;
        let current = deck(case.current, true, 1.0);
        let mut following = deck(case.following, case.matched_after_plan, case.input.speed);
        let plan = BlendPlan { start: case.plan.start, following_start: case.plan.following_start, length: case.plan.length, speed: case.plan.speed };

        let noise_bytes = fs::read(dir().join(format!("{name}.noise.f64"))).unwrap();
        let flat: Vec<f64> = noise_bytes.chunks_exact(8).map(|b| f64::from_le_bytes(b.try_into().unwrap())).collect();
        let (mut draws, mut rest) = (Vec::new(), &flat[..]);
        for &n in &case.draws {
            let (head, tail) = rest.split_at(n);
            draws.push(head.to_vec());
            rest = tail;
        }
        let noise: SharedNoise = Arc::new(Mutex::new(Replay(draws.into_iter())));

        let radio = Recorder::default();
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<radio_core::pipeline::Piece>(4);
        let collector = tokio::spawn(async move {
            let mut pieces = Vec::new();
            while let Some(piece) = receiver.recv().await {
                pieces.push(piece);
            }
            pieces
        });
        transition(&radio, &clock, &current, &mut following, &plan, case.effects, noise, &sender as &dyn radio_core::pipeline::PieceSink).await.expect("the mix is taken");
        drop(sender);
        let pieces = collector.await.unwrap();

        assert_eq!(*radio.events.lock().unwrap(), case.events, "{name}: what the radio was told");
        assert_eq!(following.beat_matched, case.beat_matched_after, "{name}: beat matched after");
        let log: Vec<(String, usize)> = pieces.iter().map(|p| (p.title.to_string(), p.pcm.len() / 2)).collect();
        assert_eq!(log, case.pieces, "{name}: the pieces");

        let report = serde_json::to_value(&radio.reports.lock().unwrap()[0]).unwrap();
        let mut report = report.as_object().unwrap().clone();
        report.remove("at");
        assert_eq!(Value::Object(report), case.report, "{name}: the report");

        let got: Vec<f32> = pieces.iter().flat_map(|p| p.pcm.iter().copied()).collect();
        let bytes = fs::read(dir().join(format!("{name}.want.f32"))).unwrap();
        let want: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
        let snr = snr_db(&got, &want);
        println!("{name:14} {:13} {snr:6.1} dB", format!("{}/{}", case.report["kind"].as_str().unwrap(), case.report["effect"].as_str().unwrap()));
        assert!(snr > 100.0, "{name}: {snr:.1} dB");
        kinds.insert(format!("{}/{}", case.report["kind"].as_str().unwrap(), case.report["effect"].as_str().unwrap()));
    }
    assert!(kinds.len() >= 5, "the cases are of {} kinds only: {kinds:?}", kinds.len());
}
