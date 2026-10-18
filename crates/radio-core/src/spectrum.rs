//! The spectrum of the mix, a few bands many times a second, for the page. Task 3.5 of docs/rust-port.md, ported
//! from `spectrum.py`.
//!
//! The radio mixes the sound itself, so it can tell how loud each band is at every moment. The page can't analyze the
//! stream it plays (that needs CORS, and it hears the sound ~3 s after the mix made it), so each value comes with its
//! position in the stream, and the page shows it when its player gets there.

use crate::mix::{CHANNELS, SAMPLE_RATE};
use realfft::RealFftPlanner;
use std::{
    collections::{HashMap, VecDeque},
    f64::consts::PI,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};
use tokio::sync::Notify;

pub const BANDS: usize = 12;
const WINDOW: usize = 2048;
/// ~30 values a second.
const HOP: usize = SAMPLE_RATE as usize / 30;
const LOW_HZ: f64 = 30.0;
const HIGH_HZ: f64 = 16000.0;
/// A band is 0 at this level (dBFS) and 1 at full scale.
const FLOOR_DB: f64 = -60.0;
/// Music falls off towards the treble: lift it so that the right bars move.
const TILT_DB_PER_OCTAVE: f64 = 3.0;
const TILT_FROM_HZ: f64 = 500.0;
/// The frames kept for the listeners that join: more than the preroll and how far ahead of the listeners the mix runs.
const HISTORY_FRAMES: usize = SAMPLE_RATE as usize / HOP * 12;
/// How many batches of frames wait for a page that does not keep up.
pub const CLIENT_QUEUE_SIZE: usize = 64;
const STARTS_KEPT: usize = 256;

/// The bands at a position in the stream (seconds of the mix).
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub t: f64,
    pub bands: [f64; BANDS],
}

/// Python's `round(value, 3)` (the decimal number nearest to the value), as the float it is written with.
fn round3(value: f64) -> f64 {
    format!("{value:.3}").parse().unwrap_or(value)
}

impl Frame {
    /// `{"t":12.345,"b":[0.1,...]}`, the numbers to 3 places, no spaces.
    pub fn to_json(&self) -> String {
        let number = |v: f64| serde_json::to_string(&round3(v)).expect("a finite number");
        let bands: Vec<String> = self.bands.iter().map(|&v| number(v)).collect();
        format!("{{\"t\":{},\"b\":[{}]}}", number(self.t), bands.join(","))
    }
}

/// Server-sent events for frames: `data: {...}` and an empty line for each.
pub fn sse_frames(frames: &[Frame]) -> String {
    frames
        .iter()
        .map(|f| format!("data: {}\n\n", f.to_json()))
        .collect()
}

/// The first event: where in the mix the player of the page starts (`null` if it did not connect in time).
pub fn sse_start(position: Option<f64>) -> String {
    let position = position.map_or_else(
        || "null".to_owned(),
        |p| serde_json::to_string(&p).expect("a finite number"),
    );
    format!("event: start\ndata: {{\"position\": {position}}}\n\n")
}

/// `np.geomspace(low, high, count)`.
fn geomspace(low: f64, high: f64, count: usize) -> Vec<f64> {
    let (log_low, log_high) = (low.log10(), high.log10());
    let step = (log_high - log_low) / (count - 1) as f64;
    let mut edges: Vec<f64> = (0..count)
        .map(|i| 10f64.powf(i as f64 * step + log_low))
        .collect();
    edges[0] = low;
    edges[count - 1] = high;
    edges
}

/// Cuts the mix into windows, as it is fed, and gives their bands.
pub struct SpectrumAnalyzer {
    buffer: Vec<f32>,
    /// The index of the first sample of the buffer in the mix.
    origin: usize,
    window: Vec<f32>,
    /// The range of bins of each band.
    ranges: Vec<(usize, usize)>,
    tilt: Vec<f64>,
    fft: Arc<dyn realfft::RealToComplex<f32>>,
}

impl Default for SpectrumAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl SpectrumAnalyzer {
    pub fn new() -> Self {
        // np.hanning: symmetric.
        let window = (0..WINDOW)
            .map(|n| (0.5 - 0.5 * (2.0 * PI * n as f64 / (WINDOW - 1) as f64).cos()) as f32)
            .collect();
        let freqs: Vec<f64> = (0..=WINDOW / 2)
            .map(|k| k as f64 * f64::from(SAMPLE_RATE) / WINDOW as f64)
            .collect();
        let edges = geomspace(LOW_HZ, HIGH_HZ, BANDS + 1);
        // np.searchsorted: the first bin at or above the edge.
        let search = |value: f64| freqs.partition_point(|&f| f < value);
        let ranges = (0..BANDS)
            .map(|band| {
                let first = search(edges[band]);
                (first, search(edges[band + 1]).max(first + 1))
            })
            .collect();
        let tilt = (0..BANDS)
            .map(|band| {
                TILT_DB_PER_OCTAVE * ((edges[band] * edges[band + 1]).sqrt() / TILT_FROM_HZ).log2()
            })
            .collect();
        Self {
            buffer: Vec::new(),
            origin: 0,
            window,
            ranges,
            tilt,
            fft: RealFftPlanner::<f32>::new().plan_fft_forward(WINDOW),
        }
    }

    /// Seconds of the mix that were fed.
    pub fn position(&self) -> f64 {
        (self.origin + self.buffer.len()) as f64 / f64::from(SAMPLE_RATE)
    }

    /// Take the next piece of the mix (stereo, interleaved), return the bands of its windows.
    pub fn feed(&mut self, pcm: &[f32]) -> Vec<Frame> {
        self.buffer
            .extend(pcm.chunks_exact(CHANNELS).map(|f| (f[0] + f[1]) / 2.0));
        if self.buffer.len() < WINDOW {
            return Vec::new();
        }
        let count = (self.buffer.len() - WINDOW) / HOP + 1;
        let mut input = self.fft.make_input_vec();
        let mut output = self.fft.make_output_vec();
        let mut frames = Vec::with_capacity(count);
        for i in 0..count {
            for ((x, &s), &w) in input
                .iter_mut()
                .zip(&self.buffer[i * HOP..i * HOP + WINDOW])
                .zip(&self.window)
            {
                *x = s * w;
            }
            self.fft
                .process(&mut input, &mut output)
                .expect("sizes are right");
            // A sine of amplitude 1 gives 1 here (the Hann window passes half).
            let amplitude: Vec<f32> = output
                .iter()
                .map(|bin| bin.norm() / (WINDOW / 4) as f32)
                .collect();
            let mut bands = [0.0f64; BANDS];
            for (band, &(low, high)) in self.ranges.iter().enumerate() {
                let peak = amplitude[low..high]
                    .iter()
                    .fold(0.0f32, |top, &a| top.max(a));
                let db = f64::from(20.0 * peak.max(1e-6).log10()) + self.tilt[band];
                bands[band] = ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0);
            }
            frames.push(Frame {
                t: ((self.origin + i * HOP) as f64 + WINDOW as f64 / 2.0) / f64::from(SAMPLE_RATE),
                bands,
            });
        }
        let used = count * HOP;
        self.buffer.drain(..used);
        self.origin += used;
        frames
    }
}

struct Page {
    queue: Mutex<VecDeque<Arc<Vec<Frame>>>>,
    ready: Notify,
}

/// A page that watches the frames.
pub struct Watcher {
    page: Arc<Page>,
    feed: Arc<SpectrumFeed>,
}

impl Watcher {
    /// The next batch of frames, waiting for it.
    pub async fn recv(&self) -> Arc<Vec<Frame>> {
        loop {
            let notified = self.page.ready.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(batch) = self.page.queue.lock().expect("not poisoned").pop_front() {
                return batch;
            }
            notified.await;
        }
    }

    /// The batch that is waiting now, if any.
    pub fn try_recv(&self) -> Option<Arc<Vec<Frame>>> {
        self.page.queue.lock().expect("not poisoned").pop_front()
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.feed.state().1.retain(|p| !Arc::ptr_eq(p, &self.page));
    }
}

/// Hands the frames to the pages that watch them.
#[derive(Default)]
pub struct SpectrumFeed {
    state: Mutex<(VecDeque<Frame>, Vec<Arc<Page>>)>,
}

impl SpectrumFeed {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn state(&self) -> MutexGuard<'_, (VecDeque<Frame>, Vec<Arc<Page>>)> {
        self.state.lock().expect("not poisoned")
    }

    /// Forget the frames: the mix starts over, its positions with it.
    pub fn reset(&self) {
        self.state().0.clear();
    }

    pub fn publish(&self, frames: Vec<Frame>) {
        if frames.is_empty() {
            return;
        }
        let mut state = self.state();
        state.0.extend(frames.iter().cloned());
        while state.0.len() > HISTORY_FRAMES {
            state.0.pop_front();
        }
        let batch = Arc::new(frames);
        for page in &state.1 {
            // A page that can't keep up loses the oldest frames.
            let mut queue = page.queue.lock().expect("not poisoned");
            if queue.len() >= CLIENT_QUEUE_SIZE {
                queue.pop_front();
            }
            queue.push_back(Arc::clone(&batch));
            drop(queue);
            page.ready.notify_one();
        }
    }

    /// Watch the frames: those from `since` on, then the new ones.
    pub fn subscribe(self: &Arc<Self>, since: Option<f64>) -> (Watcher, Vec<Frame>) {
        let page = Arc::new(Page {
            queue: Mutex::new(VecDeque::new()),
            ready: Notify::new(),
        });
        let mut state = self.state();
        state.1.push(Arc::clone(&page));
        let backlog = since.map_or_else(Vec::new, |since| {
            state.0.iter().filter(|f| f.t >= since).cloned().collect()
        });
        (
            Watcher {
                page,
                feed: Arc::clone(self),
            },
            backlog,
        )
    }
}

/// Where in the stream each listener's player starts. The page can't read the headers of the stream its `<audio>`
/// plays, so it names itself in the address and asks here (with the same name).
#[derive(Default)]
pub struct StartPositions {
    state: Mutex<Starts>,
}

#[derive(Default)]
struct Starts {
    /// The names in the order they were set, the latest last, and where each starts.
    order: VecDeque<String>,
    places: HashMap<String, f64>,
    asked: HashMap<String, Arc<Notify>>,
}

impl StartPositions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&self, name: &str, position: f64) {
        let mut state = self.state.lock().expect("not poisoned");
        if state.places.insert(name.to_owned(), position).is_some() {
            state.order.retain(|n| n != name);
        }
        state.order.push_back(name.to_owned());
        while state.order.len() > STARTS_KEPT {
            if let Some(oldest) = state.order.pop_front() {
                state.places.remove(&oldest);
            }
        }
        if let Some(asked) = state.asked.remove(name) {
            asked.notify_waiters();
        }
    }

    /// The start of a listener, waiting for it to connect.
    pub async fn get(&self, name: &str, wait: Duration) -> Option<f64> {
        let notify = {
            let mut state = self.state.lock().expect("not poisoned");
            if let Some(&place) = state.places.get(name) {
                return Some(place);
            }
            Arc::clone(
                state
                    .asked
                    .entry(name.to_owned())
                    .or_insert_with(|| Arc::new(Notify::new())),
            )
        };
        let notified = notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        // It may have been set between the look and the wait.
        if let Some(&place) = self.state.lock().expect("not poisoned").places.get(name) {
            return Some(place);
        }
        if tokio::time::timeout(wait, notified).await.is_err() {
            self.state.lock().expect("not poisoned").asked.remove(name);
        }
        self.state
            .lock()
            .expect("not poisoned")
            .places
            .get(name)
            .copied()
    }
}
