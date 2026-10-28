//! The console: two decks played by hand, in the app. Each deck holds a whole track (stereo, 44.1 kHz, fetched from the
//! radio as `/api/track/{id}/pcm`) and plays it at its own pace (the pitch fader changes the speed, as on a turntable),
//! through three bands of equalizer and a channel fader; the crossfader mixes the two. The radio goes on with its own
//! mix; the player of the stream is stopped while the console is on.
//!
//! The audio callback owns the positions and reads everything else from atomics, so the page never waits for it. The
//! page asks with `dj_set` ("a.play", "b.tempo", "xfade", ...), loads with `dj_load` and reads `dj_status` a few times a
//! second.

use std::f32::consts::PI;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::{ChannelCount, DeviceSinkBuilder, Player, SampleRate, Source};
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
    /// The waveform and the places to mix at, made when the track is in.
    overview: Mutex<Option<crate::waveform::Overview>>,
    /// Goes up whenever `pcm` is replaced.
    version: AtomicU32,
    state: AtomicU8,
    message: Mutex<String>,
    playing: AtomicBool,
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
            overview: Mutex::new(None),
            version: AtomicU32::new(0),
            state: AtomicU8::new(EMPTY),
            message: Mutex::new(String::new()),
            playing: AtomicBool::new(false),
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
    decks: [Deck; 2],
    /// 0 is all deck A, 1 all deck B.
    crossfader: Float,
    master: Float,
}

pub struct Dj {
    shared: Arc<Shared>,
    session: Mutex<Option<Sender<()>>>,
}

impl Default for Dj {
    fn default() -> Self {
        Self {
            shared: Arc::new(Shared {
                decks: [Deck::new(), Deck::new()],
                crossfader: Float::new(0.5),
                master: Float::new(0.9),
            }),
            session: Mutex::new(None),
        }
    }
}

#[derive(Serialize)]
pub struct DeckStatus {
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
    decks: [DeckStatus; 2],
}

fn deck_index(name: &str) -> Result<usize, String> {
    match name {
        "a" => Ok(0),
        "b" => Ok(1),
        _ => Err(format!("no deck {name:?}")),
    }
}

impl Dj {
    /// Switch the console on: the output opens and the decks are heard.
    pub fn start(&self) -> Result<(), String> {
        let mut session = self.session.lock().unwrap();
        if session.is_some() {
            return Ok(());
        }
        let (stop_tx, stop_rx) = channel::<()>();
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        let shared = self.shared.clone();
        std::thread::spawn(move || run_output(shared, stop_rx, ready_tx));
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => {
                *session = Some(stop_tx);
                Ok(())
            }
            Ok(Err(message)) => Err(message),
            Err(_) => Err("the audio output did not open".to_owned()),
        }
    }

    /// Switch it off. The decks keep their tracks.
    pub fn stop(&self) {
        if let Some(stop) = self.session.lock().unwrap().take() {
            let _ = stop.send(());
        }
        for deck in &self.shared.decks {
            deck.playing.store(false, Ordering::Relaxed);
        }
    }

    pub fn is_running(&self) -> bool {
        self.session.lock().unwrap().is_some()
    }

    /// Fetch a track from the radio into a deck. It stops what the deck played.
    pub fn load(&self, deck: &str, url: String, bpm: f64) -> Result<(), String> {
        let index = deck_index(deck)?;
        let shared = self.shared.clone();
        let target = &shared.decks[index];
        target.playing.store(false, Ordering::Relaxed);
        *target.overview.lock().unwrap() = None;
        target.state.store(LOADING, Ordering::SeqCst);
        std::thread::spawn(move || {
            let deck = &shared.decks[index];
            match fetch(&url) {
                Ok(pcm) => {
                    // Before the deck is ready: the page asks for the overview as soon as it is.
                    *deck.overview.lock().unwrap() = Some(crate::waveform::analyze(&pcm, bpm));
                    *deck.pcm.lock().unwrap() = Arc::new(pcm);
                    deck.version.fetch_add(1, Ordering::SeqCst);
                    deck.position.store(0f64.to_bits(), Ordering::SeqCst);
                    deck.seek.set(f32::NAN);
                    deck.cue.set(0.0);
                    deck.state.store(READY, Ordering::SeqCst);
                }
                Err(message) => {
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
            decks: [deck(&self.shared.decks[0]), deck(&self.shared.decks[1])],
        }
    }
}

/// The track from the radio: raw little-endian float32.
fn fetch(url: &str) -> Result<Vec<f32>, String> {
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

/// The thread that owns the output device (it may not leave its thread).
fn run_output(shared: Arc<Shared>, stop: Receiver<()>, ready: Sender<Result<(), String>>) {
    let sink = match DeviceSinkBuilder::from_default_device()
        .and_then(|builder| {
            builder
                .with_buffer_size(rodio::cpal::BufferSize::Default)
                .open_stream()
        })
        .or_else(|_| DeviceSinkBuilder::open_default_sink())
    {
        Ok(sink) => sink,
        Err(error) => {
            let _ = ready.send(Err(format!("no audio output: {error}")));
            return;
        }
    };
    let player = Player::connect_new(sink.mixer());
    player.append(Console::new(shared));
    let _ = ready.send(Ok(()));
    while let Err(RecvTimeoutError::Timeout) = stop.recv_timeout(Duration::from_millis(500)) {}
    player.stop();
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

struct Console {
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
                voice.position = 0.0;
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
