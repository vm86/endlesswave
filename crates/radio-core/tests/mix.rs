//! The endless mix against `mix()` of the Python radio (`tools/gen_golden.py`, `tests/golden/radio/mix.json`). Tasks
//! 3.2.4 and 3.2.5 of docs/rust-port-radio.md.
//!
//! Both run on a radio with decks in a fixed order, and the scenario tells it when the next track is asked for and when
//! the queue is rebuilt: after the n-th piece, before the next one. They must tell the same story, in the same order
//! (the decks asked for, the words to the radio, how many pieces of what), and play the same sound. The tracks are
//! decoded by ffmpeg from `tests/golden/radio/library/*.wav` (not in git); without them, or ffmpeg, the test is skipped,
//! unless `GOLDEN_REQUIRED` is set.

use async_trait::async_trait;
use radio_core::{
    clock::ManualClock,
    mix::{TrackAnalysis, decode, effects::Effect, effects::NoiseSource, plan, plan::Transition},
    pipeline::{BlendReport, Closed, Deck, DeckError, DeckSource, Piece, PieceSink, SharedNoise, mix},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

#[derive(Deserialize)]
struct TrackCase {
    title: String,
    file: String,
    analysis: TrackAnalysis,
}
#[derive(Deserialize)]
struct Scenario {
    name: String,
    order: Vec<usize>,
    limit: usize,
    actions: HashMap<String, Vec<String>>,
    log: Vec<Value>,
}
#[derive(Deserialize)]
struct Golden {
    tracks: Vec<TrackCase>,
    scenarios: Vec<Scenario>,
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/radio")
}

/// A radio with decks in a fixed order and the flags the mix looks at.
struct Script {
    decks: Vec<(String, Arc<[f32]>, TrackAnalysis)>,
    order: Vec<usize>,
    cursor: AtomicUsize,
    generation: AtomicU64,
    skip: AtomicBool,
    log: Mutex<Vec<Value>>,
}

impl Script {
    fn event(&self, text: String) {
        self.log.lock().unwrap().push(json!(["event", text]));
    }
}

#[async_trait]
impl DeckSource for Script {
    fn skip_requested(&self) -> bool {
        self.skip.load(Ordering::SeqCst)
    }
    async fn next_deck(&self, current: Option<&TrackAnalysis>) -> Result<Deck, DeckError> {
        let index = self.order[self.cursor.fetch_add(1, Ordering::SeqCst) % self.order.len()];
        let (title, pcm, analysis) = &self.decks[index];
        let speed = current.map_or(0.0, |c| plan::transition_speed(c.bpm, analysis));
        let generation = self.generation.load(Ordering::SeqCst);
        let after = current.map_or_else(|| "none".to_owned(), |c| format!("{:.3}", c.bpm));
        self.event(format!("next_deck after {after} -> {title} gen={generation}"));
        Ok(Deck {
            title: title.as_str().into(),
            pcm: pcm.clone(),
            analysis: analysis.clone(),
            beat_matched: speed != 0.0,
            track: None,
            mood: String::new(),
            speed: if speed != 0.0 { speed } else { 1.0 },
            generation,
        })
    }
    fn is_stale(&self, deck: &Deck) -> bool {
        deck.generation != self.generation.load(Ordering::SeqCst)
    }
    fn skip_taken(&self) {
        self.skip.store(false, Ordering::SeqCst);
        self.event("skip_taken".to_owned());
    }
    fn transition(&self, kind: Transition, current: &Deck, following: &Deck, effect: Effect, _report: Option<BlendReport>) {
        self.skip.store(false, Ordering::SeqCst);
        self.event(format!("transition {} {} {} -> {}", kind.name(), effect.name(), current.title, following.title));
    }
    fn started(&self, deck: &Deck) {
        self.event(format!("started {}", deck.title));
    }
    fn finished(&self, deck: &Deck) {
        self.event(format!("finished {}", deck.title));
    }
    fn user_paused(&self) -> bool {
        false
    }
    fn hold(&self, _held: bool) {}
    async fn wait_resumed(&self) {}
}

/// Takes the pieces one at a time, does what the scenario says after the n-th, and gives the tasks a turn.
struct Driver {
    script: Arc<Script>,
    actions: HashMap<usize, Vec<String>>,
    limit: usize,
    count: AtomicUsize,
    heard: Mutex<Vec<f32>>,
}

#[async_trait]
impl PieceSink for Driver {
    async fn send(&self, piece: Piece) -> Result<(), Closed> {
        let count = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        let frames = piece.pcm.len() / 2;
        self.heard.lock().unwrap().extend_from_slice(&piece.pcm);
        {
            let mut log = self.script.log.lock().unwrap();
            let title = piece.title.to_string();
            match log.last_mut() {
                Some(Value::Array(last)) if last[0] == "piece" && last[1] == title.as_str() => {
                    last[2] = (last[2].as_u64().unwrap() + 1).into();
                    last[3] = (last[3].as_u64().unwrap() + frames as u64).into();
                }
                _ => log.push(json!(["piece", title, 1, frames])),
            }
        }
        for action in self.actions.get(&count).into_iter().flatten() {
            self.script.event(format!("action {action} after piece {count}"));
            if action == "skip" {
                self.script.skip.store(true, Ordering::SeqCst);
            } else {
                self.script.generation.fetch_add(1, Ordering::SeqCst);
            }
        }
        tokio::task::yield_now().await; // the tasks that make decks get their turn
        if count >= self.limit { Err(Closed) } else { Ok(()) }
    }
}

struct NoNoise;

impl NoiseSource for NoNoise {
    fn normal(&mut self, _count: usize) -> Vec<f64> {
        panic!("no effects, no noise");
    }
}

fn snr_db(got: &[f32], want: &[f32]) -> f64 {
    assert_eq!(got.len(), want.len(), "lengths");
    let signal: f64 = want.iter().map(|&w| f64::from(w).powi(2)).sum();
    let noise: f64 = got.iter().zip(want).map(|(&g, &w)| f64::from(g - w).powi(2)).sum();
    10.0 * (signal / noise.max(1e-30)).log10()
}

#[tokio::test]
async fn the_mix_plays_and_tells_what_python_does() {
    let golden: Golden = serde_json::from_str(&fs::read_to_string(dir().join("mix.json")).unwrap()).unwrap();
    let library = dir().join("library");
    if !library.join("t0.wav").exists() {
        assert!(std::env::var_os("GOLDEN_REQUIRED").is_none(), "no library, run tools/gen_golden.py");
        eprintln!("no library: skipped");
        return;
    }
    let mut decks = Vec::new();
    for track in &golden.tracks {
        match decode::decode_file(&library.join(&track.file)) {
            Ok(pcm) => decks.push((track.title.clone(), Arc::<[f32]>::from(pcm), track.analysis.clone())),
            Err(error) => {
                assert!(std::env::var_os("GOLDEN_REQUIRED").is_none(), "{error}");
                eprintln!("{error}: skipped");
                return;
            }
        }
    }

    for scenario in &golden.scenarios {
        let name = &scenario.name;
        let script = Arc::new(Script {
            decks: decks.clone(),
            order: scenario.order.clone(),
            cursor: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
            skip: AtomicBool::new(false),
            log: Mutex::new(Vec::new()),
        });
        let driver = Driver {
            script: script.clone(),
            actions: scenario.actions.iter().map(|(k, v)| (k.parse().unwrap(), v.clone())).collect(),
            limit: scenario.limit,
            count: AtomicUsize::new(0),
            heard: Mutex::new(Vec::new()),
        };
        let noise: SharedNoise = Arc::new(Mutex::new(NoNoise));
        let ended = mix(script.clone(), Arc::new(ManualClock::new(0.0)), 0.0, noise, &driver).await;
        assert_eq!(ended, Err(radio_core::pipeline::MixError::Closed), "{name}: how it ended");

        let log = script.log.lock().unwrap().clone();
        assert_eq!(log.len(), scenario.log.len(), "{name}: the story has as many parts");
        for (n, (got, want)) in log.iter().zip(&scenario.log).enumerate() {
            assert_eq!(got, want, "{name}: part {n} of the story");
        }
        let heard = driver.heard.lock().unwrap().clone();
        let bytes = fs::read(dir().join(format!("mix_{name}.want.f32"))).unwrap();
        let want: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
        let snr = snr_db(&heard, &want);
        println!("{name:18} {} frames, {snr:6.1} dB", heard.len() / 2);
        assert!(snr > 100.0, "{name}: {snr:.1} dB");
    }
}
