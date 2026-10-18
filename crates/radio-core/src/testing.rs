//! Things for tests of the radio, here so that the tests of the crate and the tests of whoever uses it share them.

use crate::{
    mix::{
        TrackAnalysis,
        effects::Effect,
        plan::{self, Transition},
    },
    pipeline::{BlendReport, Deck, DeckError, DeckSource},
};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use tokio::sync::Notify;

/// A radio with decks in a fixed order and the flags the mix looks at, which a test sets. What the mix tells it is
/// written to `log` the way `Scripted` of `tools/gen_golden.py` writes it.
pub struct ScriptedRadio {
    pub decks: Vec<(String, Arc<[f32]>, TrackAnalysis)>,
    pub order: Vec<usize>,
    pub cursor: AtomicUsize,
    pub generation: AtomicU64,
    pub skip: AtomicBool,
    /// Paused through the API.
    pub paused: AtomicBool,
    resumed: Notify,
    /// `["event", text]` and `["piece", title, count, frames]`, in order.
    pub log: Mutex<Vec<Value>>,
    /// The values `hold` was called with.
    pub holds: Mutex<Vec<bool>>,
    /// The decks asked for and not yet made are made after this many (0: at once).
    pub fail_after: Mutex<Option<usize>>,
}

impl ScriptedRadio {
    pub fn new(decks: Vec<(String, Arc<[f32]>, TrackAnalysis)>, order: Vec<usize>) -> Arc<Self> {
        Arc::new(Self {
            decks,
            order,
            cursor: AtomicUsize::new(0),
            generation: AtomicU64::new(0),
            skip: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            resumed: Notify::new(),
            log: Mutex::new(Vec::new()),
            holds: Mutex::new(Vec::new()),
            fail_after: Mutex::new(None),
        })
    }

    pub fn event(&self, text: String) {
        self.log
            .lock()
            .expect("not poisoned")
            .push(json!(["event", text]));
    }

    /// Pause the mix, or let it go on.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
        if !paused {
            self.resumed.notify_waiters();
        }
    }
}

#[async_trait]
impl DeckSource for ScriptedRadio {
    fn skip_requested(&self) -> bool {
        self.skip.load(Ordering::SeqCst)
    }

    async fn next_deck(&self, current: Option<&TrackAnalysis>) -> Result<Deck, DeckError> {
        if let Some(left) = self.fail_after.lock().expect("not poisoned").as_mut() {
            if *left == 0 {
                return Err(DeckError("no more decks".into()));
            }
            *left -= 1;
        }
        let index = self.order[self.cursor.fetch_add(1, Ordering::SeqCst) % self.order.len()];
        let (title, pcm, analysis) = &self.decks[index];
        let speed = current.map_or(0.0, |c| plan::transition_speed(c.bpm, analysis));
        let generation = self.generation.load(Ordering::SeqCst);
        let after = current.map_or_else(|| "none".to_owned(), |c| format!("{:.3}", c.bpm));
        self.event(format!(
            "next_deck after {after} -> {title} gen={generation}"
        ));
        Ok(Deck {
            title: title.as_str().into(),
            pcm: pcm.clone(),
            analysis: analysis.clone(),
            beat_matched: speed != 0.0,
            track: None,
            mood: String::new(),
            speed: if speed != 0.0 { speed } else { 1.0 },
            generation,
            source: None,
        })
    }

    fn is_stale(&self, deck: &Deck) -> bool {
        deck.generation != self.generation.load(Ordering::SeqCst)
    }

    fn skip_taken(&self) {
        self.skip.store(false, Ordering::SeqCst);
        self.event("skip_taken".to_owned());
    }

    fn transition(
        &self,
        kind: Transition,
        current: &Deck,
        following: &Deck,
        effect: Effect,
        _report: Option<BlendReport>,
    ) {
        self.skip.store(false, Ordering::SeqCst);
        self.event(format!(
            "transition {} {} {} -> {}",
            kind.name(),
            effect.name(),
            current.title,
            following.title
        ));
    }

    fn started(&self, deck: &Deck) {
        self.event(format!("started {}", deck.title));
    }

    fn finished(&self, deck: &Deck) {
        self.event(format!("finished {}", deck.title));
    }

    fn user_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    fn hold(&self, held: bool) {
        self.holds.lock().expect("not poisoned").push(held);
    }

    async fn wait_resumed(&self) {
        loop {
            let notified = self.resumed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.paused.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
}

/// A deck of a pure tone, with an analysis made up to match (a steady tempo, no sections): enough for a mix to play it
/// alone, which is what tests of the plumbing need.
pub fn tone_deck(title: &str, seconds: f64, bpm: f64) -> (String, Arc<[f32]>, TrackAnalysis) {
    let frames = (seconds * f64::from(crate::mix::SAMPLE_RATE)) as usize;
    let pcm: Vec<f32> = (0..frames)
        .flat_map(|i| {
            let s = (0.3
                * (2.0 * std::f64::consts::PI * 440.0 * i as f64
                    / f64::from(crate::mix::SAMPLE_RATE))
                .sin()) as f32;
            [s, s]
        })
        .collect();
    let analysis = TrackAnalysis {
        bpm,
        head_beat: 1000.0,
        tail_beat: frames as f64 - 1000.0,
        start: 0,
        end: frames as i64,
        loudness_db: -14.0,
        intro_end: -1,
        outro_start: -1,
        key: 0,
    };
    (title.to_owned(), Arc::from(pcm), analysis)
}
