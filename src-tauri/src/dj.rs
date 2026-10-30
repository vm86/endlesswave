//! The console: two decks played by hand, in the app. Each deck holds a whole track (stereo, 44.1 kHz, fetched from the
//! radio as `/api/track/{id}/pcm`) and plays it at its own pace (the pitch fader changes the speed, as on a turntable),
//! through three bands of equalizer and a channel fader; the crossfader mixes the two. The console and the radio share
//! one output; changing modes transfers the playing track and its position.
//!
//! The audio callback owns the positions and reads everything else from atomics, so the page never waits for it. The
//! page asks with `dj_set` ("a.play", "b.tempo", "xfade", ...), loads with `dj_load` and reads `dj_status` a few times a
//! second.

use std::collections::HashSet;
use std::f32::consts::PI;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use radio_core::radio::{PlaybackInfo, TrackInfo};
use rodio::{ChannelCount, SampleRate, Source};
use serde::Serialize;

const RATE: u32 = 44_100;
/// The audio callback takes the commands of the page this often (in frames).
const BLOCK: usize = 256;
/// How far a band is cut or lifted (dB); the lowest is a kill.
const MIN_DB: f32 = -40.0;
const MAX_DB: f32 = 9.0;
/// The pitch fader: the speed of a deck relative to its own.
const MIN_TEMPO: f32 = 0.84;
const MAX_TEMPO: f32 = 1.16;

const EMPTY: u8 = 0;
const LOADING: u8 = 1;
const READY: u8 = 2;
const FAILED: u8 = 3;

/// Bits of an f32 that sit in an atomic.
struct Float(AtomicU32);

impl Float {
    fn new(value: f32) -> Self {
        Self(AtomicU32::new(value.to_bits()))
    }
    fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn set(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }
}

struct Deck {
    pcm: Mutex<Arc<Vec<f32>>>,
    track: Mutex<Option<TrackInfo>>,
    load_id: AtomicU64,
    load_commit: Mutex<()>,
    /// The waveform and the places to mix at, made when the track is in.
    overview: Mutex<Option<crate::waveform::Overview>>,
    /// Goes up whenever `pcm` is replaced.
    version: AtomicU32,
    state: AtomicU8,
    message: Mutex<String>,
    playing: AtomicBool,
    heard: AtomicBool,
    /// Where the deck is, in frames (an f64 in the bits of a u64), as the audio callback last left it.
    position: AtomicU64,
    /// Where the page wants it to be (seconds as f32 bits), or NaN.
    seek: Float,
    cue: Float,
    tempo: Float,
    fader: Float,
    low: Float,
    mid: Float,
    high: Float,
    /// The loudest sample of the last block, falling slowly.
    peak: Float,
}

impl Deck {
    fn new() -> Self {
        Self {
            pcm: Mutex::new(Arc::new(Vec::new())),
            track: Mutex::new(None),
            load_id: AtomicU64::new(0),
            load_commit: Mutex::new(()),
            overview: Mutex::new(None),
            version: AtomicU32::new(0),
            state: AtomicU8::new(EMPTY),
            message: Mutex::new(String::new()),
            playing: AtomicBool::new(false),
            heard: AtomicBool::new(false),
            position: AtomicU64::new(0f64.to_bits()),
            seek: Float::new(f32::NAN),
            cue: Float::new(0.0),
            tempo: Float::new(1.0),
            fader: Float::new(1.0),
            low: Float::new(0.0),
            mid: Float::new(0.0),
            high: Float::new(0.0),
            peak: Float::new(0.0),
        }
    }

    fn frames(&self) -> usize {
        self.pcm.lock().unwrap().len() / 2
    }

    fn position(&self) -> f64 {
        f64::from_bits(self.position.load(Ordering::Relaxed))
    }
}

struct Shared {
    running: AtomicBool,
    heard_tracks: Mutex<HashSet<String>>,
    decks: [Deck; 2],
    /// 0 is all deck A, 1 all deck B.
    crossfader: Float,
    master: Float,
}

pub struct Dj {
    shared: Arc<Shared>,
    pub(crate) handoff: Mutex<()>,
    lease_owner: String,
}

impl Default for Dj {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared {
                running: AtomicBool::new(false),
                heard_tracks: Mutex::new(HashSet::new()),
                decks: [Deck::new(), Deck::new()],
                crossfader: Float::new(0.5),
                master: Float::new(0.9),
            }),
            handoff: Mutex::new(()),
            lease_owner: uuid::Uuid::new_v4().to_string(),
        }
    }
}

#[derive(Serialize)]
pub struct DeckStatus {
    track: Option<TrackInfo>,
    overview_ready: bool,
    low: f32,
    mid: f32,
    high: f32,
    fader: f32,
    state: &'static str,
    message: String,
    playing: bool,
    /// Seconds.
    position: f64,
    duration: f64,
    cue: f64,
    tempo: f32,
    peak: f32,
}

#[derive(Serialize)]
pub struct Status {
    running: bool,
    crossfader: f32,
    master: f32,
    decks: [DeckStatus; 2],
}

#[derive(Clone)]
pub(crate) struct ReturnPoint {
    pub track: TrackInfo,
    pub next: Option<TrackInfo>,
    pub position: f64,
    pub playing: bool,
    pub tempo: f64,
}

fn deck_index(name: &str) -> Result<usize, String> {
    match name {
        "a" => Ok(0),
        "b" => Ok(1),
        _ => Err(format!("no deck {name:?}")),
    }
}

impl Dj {
    pub(crate) fn source(&self) -> Console {
        Console::new(self.shared.clone())
    }

    pub fn stop(&self) {
        self.shared.running.store(false, Ordering::SeqCst);
        self.pause();
    }

    pub fn pause(&self) {
        for deck in &self.shared.decks {
            deck.playing.store(false, Ordering::Relaxed);
        }
    }

    pub fn play(&self) {
        if let Some(point) = self.return_point() {
            for deck in &self.shared.decks {
                if deck
                    .track
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|t| t.track_id == point.track.track_id)
                {
                    deck.playing.store(true, Ordering::Relaxed);
                }
            }
        }
    }

    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::SeqCst)
    }
    pub fn master(&self) -> f32 {
        self.shared.master.get()
    }
    pub(crate) fn lease_owner(&self) -> &str {
        &self.lease_owner
    }
    pub(crate) fn heard_tracks(&self) -> Vec<String> {
        let mut ids: Vec<_> = self
            .shared
            .heard_tracks
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect();
        ids.sort();
        ids
    }
    fn tag_url(&self, index: usize, version: u64, url: &str) -> String {
        format!(
            "{url}{}dj_owner={}&dj_slot={index}&dj_version={version}",
            if url.contains('?') { "&" } else { "?" },
            self.lease_owner
        )
    }
    pub(crate) fn reserve_url(&self, index: usize, url: &str) -> String {
        let version = self.shared.decks[index]
            .load_id
            .fetch_add(1, Ordering::SeqCst)
            + 1;
        self.tag_url(index, version, url)
    }

    pub(crate) fn return_point(&self) -> Option<ReturnPoint> {
        return_point(&self.shared)
    }

    pub(crate) fn prepare(
        &self,
        info: &PlaybackInfo,
        a: Vec<f32>,
        b: Option<Vec<f32>>,
        volume: f32,
    ) {
        *self.shared.heard_tracks.lock().unwrap() = HashSet::from([info.track.track_id.clone()]);
        self.install(0, info.track.clone(), a, info.position);
        if let (Some(track), Some(pcm)) = (info.next.clone(), b) {
            self.install(1, track, pcm, 0.0);
        } else {
            self.clear(1);
        }
        self.shared.crossfader.set(0.0);
        self.shared.master.set(volume);
    }

    pub(crate) fn prepare_empty(&self, volume: f32) {
        self.shared.heard_tracks.lock().unwrap().clear();
        self.clear(0);
        self.clear(1);
        self.shared.crossfader.set(0.0);
        self.shared.master.set(volume);
    }

    fn clear(&self, index: usize) {
        let deck = &self.shared.decks[index];
        let _commit = deck.load_commit.lock().unwrap();
        deck.load_id.fetch_add(1, Ordering::SeqCst);
        deck.playing.store(false, Ordering::Relaxed);
        *deck.track.lock().unwrap() = None;
        *deck.pcm.lock().unwrap() = Arc::new(Vec::new());
        *deck.overview.lock().unwrap() = None;
        deck.position.store(0f64.to_bits(), Ordering::SeqCst);
        deck.version.fetch_add(1, Ordering::SeqCst);
        deck.state.store(EMPTY, Ordering::SeqCst);
    }

    fn install(&self, index: usize, track: TrackInfo, pcm: Vec<f32>, position: f64) {
        let deck = &self.shared.decks[index];
        let _commit = deck.load_commit.lock().unwrap();
        let load = deck.load_id.fetch_add(1, Ordering::SeqCst) + 1;
        let pcm = Arc::new(pcm);
        deck.playing.store(false, Ordering::Relaxed);
        *deck.track.lock().unwrap() = Some(track.clone());
        *deck.overview.lock().unwrap() = None;
        *deck.pcm.lock().unwrap() = pcm.clone();
        deck.position
            .store((position * RATE as f64).to_bits(), Ordering::SeqCst);
        deck.heard.store(position > 0.0, Ordering::Relaxed);
        deck.seek.set(position as f32);
        deck.cue.set(0.0);
        deck.tempo.set(1.0);
        deck.fader.set(1.0);
        deck.low.set(0.0);
        deck.mid.set(0.0);
        deck.high.set(0.0);
        *deck.message.lock().unwrap() = String::new();
        deck.version.fetch_add(1, Ordering::SeqCst);
        deck.state.store(READY, Ordering::SeqCst);
        let shared = self.shared.clone();
        std::thread::spawn(move || {
            let overview = crate::waveform::analyze(&pcm, track.bpm);
            let deck = &shared.decks[index];
            let _commit = deck.load_commit.lock().unwrap();
            if deck.load_id.load(Ordering::SeqCst) == load {
                *deck.overview.lock().unwrap() = Some(overview);
            }
        });
    }

    /// Fetch a track from the radio into a deck. It stops what the deck played.
    pub fn load(&self, deck: &str, url: String, track: TrackInfo) -> Result<(), String> {
        let index = deck_index(deck)?;
        let shared = self.shared.clone();
        let target = &shared.decks[index];
        let load = {
            let _commit = target.load_commit.lock().unwrap();
            target.playing.store(false, Ordering::Relaxed);
            target.heard.store(false, Ordering::Relaxed);
            let load = target.load_id.fetch_add(1, Ordering::SeqCst) + 1;
            *target.track.lock().unwrap() = Some(track.clone());
            *target.overview.lock().unwrap() = None;
            target.state.store(LOADING, Ordering::SeqCst);
            load
        };
        let url = self.tag_url(index, load, &url);
        std::thread::spawn(move || {
            let deck = &shared.decks[index];
            let result = fetch(&url);
            if deck.load_id.load(Ordering::SeqCst) != load {
                return;
            }
            match result {
                Ok(pcm) => {
                    let overview = crate::waveform::analyze(&pcm, track.bpm);
                    let _commit = deck.load_commit.lock().unwrap();
                    if deck.load_id.load(Ordering::SeqCst) != load {
                        return;
                    }
                    *deck.overview.lock().unwrap() = Some(overview);
                    *deck.pcm.lock().unwrap() = Arc::new(pcm);
                    deck.version.fetch_add(1, Ordering::SeqCst);
                    deck.position.store(0f64.to_bits(), Ordering::SeqCst);
                    deck.seek.set(f32::NAN);
                    deck.cue.set(0.0);
                    deck.state.store(READY, Ordering::SeqCst);
                }
                Err(message) => {
                    let _commit = deck.load_commit.lock().unwrap();
                    if deck.load_id.load(Ordering::SeqCst) != load {
                        return;
                    }
                    *deck.message.lock().unwrap() = message;
                    deck.state.store(FAILED, Ordering::SeqCst);
                }
            }
        });
        Ok(())
    }

    /// One control of the console, by name: "xfade", "master", or "<deck>.<what>" (play, tempo, fader, low, mid, high,
    /// seek, cue, setcue).
    pub fn set(&self, name: &str, value: f32) -> Result<(), String> {
        let value = if value.is_finite() { value } else { 0.0 };
        match name {
            "xfade" => {
                self.shared.crossfader.set(value.clamp(0.0, 1.0));
                return Ok(());
            }
            "master" => {
                self.shared.master.set(value.clamp(0.0, 1.0));
                return Ok(());
            }
            _ => {}
        }
        let (which, what) = name
            .split_once('.')
            .ok_or_else(|| format!("unknown control {name:?}"))?;
        let deck = &self.shared.decks[deck_index(which)?];
        match what {
            "play" => {
                if value > 0.5 && deck.state.load(Ordering::SeqCst) == READY {
                    // Played to the end, it begins again.
                    if deck.position() >= deck.frames() as f64 {
                        deck.seek.set(0.0);
                    }
                    deck.playing.store(true, Ordering::Relaxed);
                } else {
                    deck.playing.store(false, Ordering::Relaxed);
                }
            }
            "tempo" => deck.tempo.set(value.clamp(MIN_TEMPO, MAX_TEMPO)),
            "fader" => deck.fader.set(value.clamp(0.0, 1.0)),
            "low" => deck.low.set(value.clamp(MIN_DB, MAX_DB)),
            "mid" => deck.mid.set(value.clamp(MIN_DB, MAX_DB)),
            "high" => deck.high.set(value.clamp(MIN_DB, MAX_DB)),
            "seek" => deck.seek.set(value.max(0.0)),
            // Cue: back to the cue point, and stopped there.
            "cue" => {
                deck.playing.store(false, Ordering::Relaxed);
                deck.seek.set(deck.cue.get());
            }
            "setcue" => deck.cue.set((deck.position() / RATE as f64) as f32),
            _ => return Err(format!("unknown control {name:?}")),
        }
        Ok(())
    }

    /// The waveform and the markers of the track on a deck, once it is loaded.
    pub fn overview(&self, deck: &str) -> Result<Option<crate::waveform::Overview>, String> {
        Ok(self.shared.decks[deck_index(deck)?]
            .overview
            .lock()
            .unwrap()
            .clone())
    }

    pub fn status(&self) -> Status {
        let deck = |d: &Deck| DeckStatus {
            track: d.track.lock().unwrap().clone(),
            overview_ready: d.overview.lock().unwrap().is_some(),
            low: d.low.get(),
            mid: d.mid.get(),
            high: d.high.get(),
            fader: d.fader.get(),
            state: match d.state.load(Ordering::SeqCst) {
                LOADING => "loading",
                READY => "ready",
                FAILED => "error",
                _ => "empty",
            },
            message: d.message.lock().unwrap().clone(),
            playing: d.playing.load(Ordering::Relaxed),
            position: d.position() / RATE as f64,
            duration: d.frames() as f64 / RATE as f64,
            cue: d.cue.get() as f64,
            tempo: d.tempo.get(),
            peak: d.peak.get(),
        };
        Status {
            running: self.is_running(),
            crossfader: self.shared.crossfader.get(),
            master: self.shared.master.get(),
            decks: [deck(&self.shared.decks[0]), deck(&self.shared.decks[1])],
        }
    }
}

/// The track from the radio: raw little-endian float32.
pub(crate) fn fetch(url: &str) -> Result<Vec<f32>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(180)))
        .build()
        .into();
    let mut response = agent.get(url).call().map_err(|e| e.to_string())?;
    let bytes = response
        .body_mut()
        .with_config()
        .limit(1 << 30)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    if bytes.len() < 8 {
        return Err("the radio sent no audio".to_owned());
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

fn return_point(shared: &Shared) -> Option<ReturnPoint> {
    let x = shared.crossfader.get();
    let cross = [(x * PI / 2.0).cos().max(0.0), (x * PI / 2.0).sin()];
    let selected = (0..2)
        .filter(|&i| shared.decks[i].state.load(Ordering::SeqCst) == READY)
        .filter(|&i| shared.decks[i].track.lock().unwrap().is_some())
        .max_by(|&a, &b| {
            let score = |i: usize| {
                let level = cross[i] * shared.decks[i].fader.get();
                level
                    + if level > 0.001 && shared.decks[i].playing.load(Ordering::Relaxed) {
                        2.0
                    } else {
                        0.0
                    }
            };
            score(a).total_cmp(&score(b))
        })?;
    let deck = &shared.decks[selected];
    let track = deck.track.lock().unwrap().clone()?;
    let other = &shared.decks[1 - selected];
    let next =
        if other.state.load(Ordering::SeqCst) == READY && !other.heard.load(Ordering::Relaxed) {
            other
                .track
                .lock()
                .unwrap()
                .clone()
                .filter(|t| t.track_id != track.track_id)
        } else {
            None
        };
    Some(ReturnPoint {
        track,
        next,
        position: deck.position() / RATE as f64,
        playing: deck.playing.load(Ordering::Relaxed) && cross[selected] * deck.fader.get() > 0.001,
        tempo: deck.tempo.get() as f64,
    })
}

/// One band: a shelf at the ends, a peak in the middle (the Audio EQ Cookbook). Flat until it is set.
#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Shape {
    LowShelf,
    Peak,
    HighShelf,
}

impl Default for Biquad {
    fn default() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        }
    }
}

impl Biquad {
    fn design(shape: Shape, frequency: f32, gain_db: f32) -> Self {
        if gain_db.abs() < 0.05 {
            return Self::default();
        }
        let a = 10f32.powf(gain_db / 40.0);
        let w = 2.0 * PI * frequency / RATE as f32;
        let (sin, cos) = (w.sin(), w.cos());
        let (b0, b1, b2, a0, a1, a2);
        match shape {
            Shape::Peak => {
                let alpha = sin / (2.0 * 0.9);
                b0 = 1.0 + alpha * a;
                b1 = -2.0 * cos;
                b2 = 1.0 - alpha * a;
                a0 = 1.0 + alpha / a;
                a1 = -2.0 * cos;
                a2 = 1.0 - alpha / a;
            }
            Shape::LowShelf | Shape::HighShelf => {
                // A shelf of slope 1.
                let alpha = sin / 2.0 * 2f32.sqrt();
                let beta = 2.0 * a.sqrt() * alpha;
                if shape == Shape::LowShelf {
                    b0 = a * ((a + 1.0) - (a - 1.0) * cos + beta);
                    b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos);
                    b2 = a * ((a + 1.0) - (a - 1.0) * cos - beta);
                    a0 = (a + 1.0) + (a - 1.0) * cos + beta;
                    a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos);
                    a2 = (a + 1.0) + (a - 1.0) * cos - beta;
                } else {
                    b0 = a * ((a + 1.0) + (a - 1.0) * cos + beta);
                    b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos);
                    b2 = a * ((a + 1.0) + (a - 1.0) * cos - beta);
                    a0 = (a + 1.0) - (a - 1.0) * cos + beta;
                    a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos);
                    a2 = (a + 1.0) - (a - 1.0) * cos - beta;
                }
            }
        }
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// New coefficients, the state of the filter kept (no click when a knob is turned).
    fn retune(&mut self, shape: Shape, frequency: f32, gain_db: f32) {
        let new = Self::design(shape, frequency, gain_db);
        *self = Self {
            z1: self.z1,
            z2: self.z2,
            ..new
        };
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// Linear up to 0.8, then bent smoothly towards 1.
fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let magnitude = x.abs();
    if magnitude <= KNEE {
        return x;
    }
    x.signum() * (KNEE + (1.0 - KNEE) * ((magnitude - KNEE) / (1.0 - KNEE)).tanh())
}

/// What the console keeps of one deck inside the audio callback.
struct Voice {
    pcm: Arc<Vec<f32>>,
    version: u32,
    position: f64,
    /// The filters of the three bands, for each channel.
    bands: [[Biquad; 3]; 2],
    gains: [f32; 3],
    /// The channel fader and the crossfader as they were heard (smoothed against clicks).
    level: f32,
}

impl Voice {
    fn new() -> Self {
        Self {
            pcm: Arc::new(Vec::new()),
            version: u32::MAX,
            position: 0.0,
            bands: [[Biquad::default(); 3]; 2],
            gains: [0.0; 3],
            level: 0.0,
        }
    }
}

const BAND_SHAPES: [(Shape, f32); 3] = [
    (Shape::LowShelf, 250.0),
    (Shape::Peak, 1_200.0),
    (Shape::HighShelf, 4_000.0),
];

pub(crate) struct Console {
    shared: Arc<Shared>,
    voices: [Voice; 2],
    /// The frames of the block that is being played.
    block: Vec<f32>,
    at: usize,
    master: f32,
}

impl Console {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
            voices: [Voice::new(), Voice::new()],
            block: Vec::new(),
            at: 0,
            master: 0.0,
        }
    }

    pub(crate) fn enter(&mut self, position: f64, tempo: f64, playing: bool) {
        let deck = &self.shared.decks[0];
        deck.seek.set(f32::NAN);
        deck.position
            .store((position * RATE as f64).to_bits(), Ordering::SeqCst);
        self.voices[0].version = u32::MAX;
        deck.tempo.set((tempo as f32).clamp(MIN_TEMPO, MAX_TEMPO));
        deck.playing.store(playing, Ordering::SeqCst);
        self.shared.decks[1].playing.store(false, Ordering::SeqCst);
        self.shared.running.store(true, Ordering::SeqCst);
        self.master = self.shared.master.get();
        let x = self.shared.crossfader.get();
        self.voices[0].level = self.shared.decks[0].fader.get() * (x * PI / 2.0).cos();
        self.voices[1].level = self.shared.decks[1].fader.get() * (x * PI / 2.0).sin();
        self.block.clear();
        self.at = 0;
    }

    pub(crate) fn leave(&mut self) {
        self.shared.running.store(false, Ordering::SeqCst);
        for deck in &self.shared.decks {
            deck.playing.store(false, Ordering::Relaxed);
        }
    }

    pub(crate) fn return_point(&self) -> Option<ReturnPoint> {
        return_point(&self.shared)
    }

    /// Mix the next `BLOCK` frames.
    fn fill(&mut self) {
        let shared = self.shared.clone();
        self.block.clear();
        self.block.resize(BLOCK * 2, 0.0);
        let x = shared.crossfader.get();
        // Equal power: the middle is not quieter than the ends.
        let cross = [(x * PI / 2.0).cos(), (x * PI / 2.0).sin()];
        let master_target = shared.master.get();
        for (index, (voice, deck)) in self.voices.iter_mut().zip(&shared.decks).enumerate() {
            let version = deck.version.load(Ordering::SeqCst);
            if voice.version != version {
                voice.pcm = deck.pcm.lock().unwrap().clone();
                voice.version = version;
                voice.position = deck.position();
                voice.bands = [[Biquad::default(); 3]; 2];
            }
            let seek = deck.seek.get();
            if !seek.is_nan() {
                deck.seek.set(f32::NAN);
                voice.position =
                    (seek as f64 * RATE as f64).clamp(0.0, (voice.pcm.len() / 2) as f64);
            }
            let wanted = [deck.low.get(), deck.mid.get(), deck.high.get()];
            if wanted != voice.gains {
                voice.gains = wanted;
                for channel in &mut voice.bands {
                    for (i, band) in channel.iter_mut().enumerate() {
                        band.retune(BAND_SHAPES[i].0, BAND_SHAPES[i].1, wanted[i]);
                    }
                }
            }
            let frames = voice.pcm.len() / 2;
            let playing = deck.playing.load(Ordering::Relaxed) && frames > 0;
            let tempo = deck.tempo.get() as f64;
            let target = deck.fader.get() * cross[index];
            let mut peak = 0.0f32;
            for n in 0..BLOCK {
                voice.level += (target - voice.level) * 0.004;
                if !playing || voice.position >= (frames - 1) as f64 {
                    if playing {
                        deck.playing.store(false, Ordering::Relaxed);
                        voice.position = frames as f64;
                    }
                    break;
                }
                if voice.level.abs() > 0.001
                    && !deck.heard.swap(true, Ordering::Relaxed)
                    && let Some(track) = deck.track.lock().unwrap().as_ref()
                {
                    self.shared
                        .heard_tracks
                        .lock()
                        .unwrap()
                        .insert(track.track_id.clone());
                }
                let at = voice.position as usize;
                let fraction = (voice.position - at as f64) as f32;
                for channel in 0..2 {
                    let (a, b) = (
                        voice.pcm[at * 2 + channel],
                        voice.pcm[(at + 1) * 2 + channel],
                    );
                    let mut sample = a + (b - a) * fraction;
                    for band in &mut voice.bands[channel] {
                        sample = band.run(sample);
                    }
                    peak = peak.max(sample.abs());
                    self.block[n * 2 + channel] += sample * voice.level;
                }
                voice.position += tempo;
            }
            deck.position
                .store(voice.position.to_bits(), Ordering::Relaxed);
            let fallen = deck.peak.get() * 0.85;
            deck.peak.set(peak.max(fallen));
        }
        for n in 0..BLOCK {
            self.master += (master_target - self.master) * 0.004;
            for channel in 0..2 {
                // Two loud decks add up past full scale: the peaks are rounded off, not cut.
                let sample = &mut self.block[n * 2 + channel];
                *sample = soft_clip(*sample * self.master);
            }
        }
    }
}

impl Iterator for Console {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.at >= self.block.len() {
            self.fill();
            self.at = 0;
        }
        let sample = self.block[self.at];
        self.at += 1;
        Some(sample)
    }
}

impl Source for Console {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        NonZero::new(2).unwrap()
    }
    fn sample_rate(&self) -> SampleRate {
        NonZero::new(RATE).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn console_with(a: Vec<f32>, b: Vec<f32>) -> (Console, Arc<Shared>) {
        let shared = Arc::new(Shared {
            running: AtomicBool::new(false),
            heard_tracks: Mutex::new(HashSet::new()),
            decks: [Deck::new(), Deck::new()],
            crossfader: Float::new(0.0),
            master: Float::new(1.0),
        });
        for (deck, pcm) in shared.decks.iter().zip([a, b]) {
            *deck.pcm.lock().unwrap() = Arc::new(pcm);
            deck.version.fetch_add(1, Ordering::SeqCst);
            deck.state.store(READY, Ordering::SeqCst);
        }
        (Console::new(shared.clone()), shared)
    }

    /// A tone of one frequency, both channels.
    fn tone(frequency: f32, seconds: f32) -> Vec<f32> {
        (0..(RATE as f32 * seconds) as usize)
            .flat_map(|n| {
                let s = 0.5 * (2.0 * PI * frequency * n as f32 / RATE as f32).sin();
                [s, s]
            })
            .collect()
    }

    fn energy(console: &mut Console, blocks: usize) -> f32 {
        let mut total = 0.0;
        for _ in 0..blocks {
            console.fill();
            total += console.block.iter().map(|s| s * s).sum::<f32>();
        }
        total
    }

    #[test]
    fn a_stopped_deck_is_silent_and_a_playing_one_is_heard() {
        let (mut console, shared) = console_with(tone(440.0, 2.0), tone(440.0, 2.0));
        assert_eq!(energy(&mut console, 20), 0.0);
        shared.decks[0].playing.store(true, Ordering::Relaxed);
        assert!(energy(&mut console, 200) > 1.0);
    }

    #[test]
    fn the_crossfader_picks_the_deck() {
        let (mut console, shared) = console_with(tone(440.0, 10.0), tone(440.0, 10.0));
        shared.decks[0].playing.store(true, Ordering::Relaxed);
        shared.decks[1].playing.store(true, Ordering::Relaxed);
        // All the way to B: deck A is not heard (once the smoothing has settled).
        shared.crossfader.set(1.0);
        energy(&mut console, 400);
        let b_only = energy(&mut console, 50);
        shared.decks[1].playing.store(false, Ordering::Relaxed);
        energy(&mut console, 400);
        assert!(energy(&mut console, 50) < b_only * 0.001);
    }

    #[test]
    fn a_killed_low_band_removes_a_bass_tone() {
        let (mut flat, shared_flat) = console_with(tone(60.0, 3.0), Vec::new());
        shared_flat.decks[0].playing.store(true, Ordering::Relaxed);
        let before = energy(&mut flat, 300);
        let (mut cut, shared_cut) = console_with(tone(60.0, 3.0), Vec::new());
        shared_cut.decks[0].playing.store(true, Ordering::Relaxed);
        shared_cut.decks[0].low.set(MIN_DB);
        let after = energy(&mut cut, 300);
        assert!(after < before * 0.05, "{after} vs {before}");
    }

    #[test]
    fn the_sum_of_two_decks_is_rounded_off_below_full_scale() {
        assert_eq!(soft_clip(0.5), 0.5);
        assert!(soft_clip(1.4) < 1.0 && soft_clip(1.4) > soft_clip(1.0));
        assert_eq!(soft_clip(-1.4), -soft_clip(1.4));
    }

    #[test]
    fn the_tempo_moves_the_position_faster() {
        let (mut console, shared) = console_with(tone(440.0, 5.0), Vec::new());
        shared.decks[0].playing.store(true, Ordering::Relaxed);
        shared.decks[0].tempo.set(1.1);
        for _ in 0..100 {
            console.fill();
        }
        let frames = shared.decks[0].position();
        assert!(
            (frames - 100.0 * BLOCK as f64 * 1.1).abs() < 1.0,
            "{frames}"
        );
    }

    #[test]
    fn a_deck_stops_at_the_end_and_a_seek_goes_back() {
        let (mut console, shared) = console_with(tone(440.0, 0.1), Vec::new());
        shared.decks[0].playing.store(true, Ordering::Relaxed);
        for _ in 0..40 {
            console.fill();
        }
        assert!(!shared.decks[0].playing.load(Ordering::Relaxed));
        shared.decks[0].seek.set(0.0);
        console.fill();
        assert!(shared.decks[0].position() < BLOCK as f64);
    }
}
