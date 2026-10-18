//! Fans the encoded stream out to the connected listeners: a new one gets the latest seconds of it at once. Task 3.3
//! of docs/rust-port.md, ported from `Broadcaster` of `dj.py`.

use super::frames::NotThisStream;
use bytes::Bytes;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::sync::Notify;

/// A new listener gets this much of the latest audio at once, so that the player has something to start on (browsers
/// wait for ~1.5 s of audio before they play) instead of waiting for it to arrive at the pace it plays.
pub const PREROLL_SECONDS: f64 = 3.0;
/// What a second of the 320 kbit/s MP3 takes.
pub const MP3_BYTES_PER_SECOND: usize = 320 * 1000 / 8;
/// What a second of FLAC takes about (it follows the music): sizes the preroll.
pub const FLAC_BYTES_PER_SECOND: usize = 110_000;
/// How many chunks wait for a listener that does not keep up; the oldest go first.
pub const CLIENT_QUEUE_SIZE: usize = 256;
/// The rate of a stream is taken from at least this much of it.
const MIN_RATE_SPAN_SECONDS: f64 = 1.0;
/// How far what an encoder has written is behind what it was fed (its own delay, and a frame that waits to be filled),
/// measured by decoding the streams and looking for clicks of a known position.
pub const MP3_LAG_SECONDS: f64 = 0.136;
pub const FLAC_LAG_SECONDS: f64 = 0.077;
pub const OGG_LAG_SECONDS: f64 = 0.076;

/// How a stream is cut into pieces a listener can start on, and what the stream starts with.
#[derive(Clone, Copy)]
pub struct Config {
    pub preroll_seconds: f64,
    pub bytes_per_second: usize,
    /// Where the first whole frame in the data starts (`frames::mp3_start`, ...).
    pub frame_start: fn(&[u8]) -> usize,
    /// For a stream that starts with a header (FLAC, Ogg): how long it is, `Ok(None)` while it is not all there.
    pub header_length: Option<fn(&[u8]) -> Result<Option<usize>, NotThisStream>>,
    pub lag_seconds: f64,
}

impl Config {
    /// The 320 kbit/s MP3 stream.
    pub fn mp3() -> Self {
        Self {
            preroll_seconds: PREROLL_SECONDS,
            bytes_per_second: MP3_BYTES_PER_SECOND,
            frame_start: super::frames::mp3_start,
            header_length: None,
            lag_seconds: MP3_LAG_SECONDS,
        }
    }

    /// The lossless stream, FLAC.
    pub fn flac() -> Self {
        Self {
            bytes_per_second: FLAC_BYTES_PER_SECOND,
            frame_start: super::frames::flac_start,
            header_length: Some(super::frames::flac_header_length),
            lag_seconds: FLAC_LAG_SECONDS,
            ..Self::mp3()
        }
    }

    /// The same FLAC in Ogg pages, for Safari and every browser on iOS.
    pub fn ogg() -> Self {
        Self {
            bytes_per_second: FLAC_BYTES_PER_SECOND,
            frame_start: super::frames::ogg_start,
            header_length: Some(super::frames::ogg_header_length),
            lag_seconds: OGG_LAG_SECONDS,
            ..Self::mp3()
        }
    }
}

/// What waits for one listener.
struct Client {
    queue: Mutex<VecDeque<Bytes>>,
    ready: Notify,
}

#[derive(Default)]
struct State {
    title: String,
    /// The header of a stream that starts with one, sent first to the listeners that join.
    header: Vec<u8>,
    clients: Vec<Arc<Client>>,
    /// The latest audio, for the listeners that join.
    recent: VecDeque<Bytes>,
    recent_size: usize,
    /// Where in the mix the stream is: the seconds of it that were fed to the encoder (set by whoever feeds it).
    fed: f64,
    total: usize,
    /// For each recent chunk, the bytes published up to its end and the seconds then.
    marks: VecDeque<(usize, f64)>,
}

pub struct Broadcaster {
    config: Config,
    preroll: usize,
    state: Mutex<State>,
    first_listener: Notify,
}

/// A listener: what it is sent, in order. Leaving the stream is dropping it.
pub struct Listener {
    client: Arc<Client>,
    hub: Arc<Broadcaster>,
}

impl Listener {
    /// The next piece of the stream, waiting for it.
    pub async fn recv(&self) -> Bytes {
        loop {
            let notified = self.client.ready.notified();
            tokio::pin!(notified);
            // Register before looking, so that a chunk that comes between the look and the wait is not missed.
            notified.as_mut().enable();
            if let Some(chunk) = self.client.queue.lock().expect("not poisoned").pop_front() {
                return chunk;
            }
            notified.await;
        }
    }

    /// The pieces that are waiting now, without waiting for more.
    pub fn try_recv(&self) -> Option<Bytes> {
        self.client.queue.lock().expect("not poisoned").pop_front()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.hub.state().clients.retain(|c| !Arc::ptr_eq(c, &self.client));
    }
}

impl Broadcaster {
    pub fn new(config: Config) -> Arc<Self> {
        Arc::new(Self {
            preroll: (config.preroll_seconds * config.bytes_per_second as f64).round_ties_even() as usize,
            config,
            state: Mutex::new(State::default()),
            first_listener: Notify::new(),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("not poisoned")
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The title of what plays (for the ICY metadata).
    pub fn title(&self) -> String {
        self.state().title.clone()
    }

    pub fn set_title(&self, title: &str) {
        self.state().title = title.to_owned();
    }

    pub fn header(&self) -> Vec<u8> {
        self.state().header.clone()
    }

    /// Set the header that listeners get first. A listener that joined before it was there (the first one comes while the
    /// first tracks are being prepared) and has had nothing yet gets it now: without it the stream can't be played.
    /// (The Python radio leaves such a listener without the header.)
    pub fn set_header(&self, header: Vec<u8>) {
        let mut state = self.state();
        if state.total == 0 && !header.is_empty() {
            for client in &state.clients {
                let mut queue = client.queue.lock().expect("not poisoned");
                if queue.is_empty() {
                    queue.push_back(Bytes::copy_from_slice(&header));
                    drop(queue);
                    client.ready.notify_one();
                }
            }
        }
        state.header = header;
    }

    /// The seconds of the mix that were fed to the encoder.
    pub fn fed(&self) -> f64 {
        self.state().fed
    }

    pub fn set_fed(&self, seconds: f64) {
        self.state().fed = seconds;
    }

    /// Forget where the stream was: the mix starts over.
    pub fn reset_position(&self) {
        let mut state = self.state();
        state.fed = 0.0;
        state.marks.clear();
    }

    pub fn has_listeners(&self) -> bool {
        !self.state().clients.is_empty()
    }

    /// The size of the latest audio kept for the listeners that join.
    pub fn buffered_bytes(&self) -> usize {
        self.state().recent_size
    }

    /// Wait until there is a listener.
    pub async fn wait_for_listeners(&self) {
        loop {
            let notified = self.first_listener.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.has_listeners() {
                return;
            }
            notified.await;
        }
    }

    /// Wait until this stream or any of the others has a listener.
    pub async fn wait_for_a_listener(&self, others: &[&Broadcaster]) {
        let hubs: Vec<&Broadcaster> = std::iter::once(self).chain(others.iter().copied()).collect();
        while !hubs.iter().any(|hub| hub.has_listeners()) {
            let waits: Vec<_> = hubs.iter().map(|hub| Box::pin(hub.wait_for_listeners())).collect();
            futures_util::future::select_all(waits).await;
        }
    }

    /// The position in the mix of a byte of the stream. The end of what was published is where the mix was fed to; the
    /// bytes before it take as long as the stream takes per byte lately (a FLAC stream follows the music, so it is not
    /// the nominal rate). `None` if nothing was published.
    pub fn position_of(&self, offset: usize) -> Option<f64> {
        self.position_in(&self.state(), offset)
    }

    fn position_in(&self, state: &State, offset: usize) -> Option<f64> {
        let (end, seconds) = *state.marks.back()?;
        let (first_end, first_seconds) = *state.marks.front()?;
        let lately = seconds - first_seconds;
        let rate = if lately >= MIN_RATE_SPAN_SECONDS && end > first_end {
            (end - first_end) as f64 / lately
        } else {
            self.config.bytes_per_second as f64
        };
        Some(seconds - self.config.lag_seconds - (end as f64 - offset as f64) / rate)
    }

    /// Join the stream; the latest seconds of it come at once.
    pub fn subscribe(self: &Arc<Self>) -> Listener {
        self.subscribe_at().0
    }

    /// Join the stream, and say where in the mix it starts for you.
    pub fn subscribe_at(self: &Arc<Self>) -> (Listener, Option<f64>) {
        let client = Arc::new(Client { queue: Mutex::new(VecDeque::new()), ready: Notify::new() });
        let mut state = self.state();
        {
            let mut queue = client.queue.lock().expect("not poisoned");
            if !state.header.is_empty() {
                queue.push_back(Bytes::copy_from_slice(&state.header));
            }
        }
        // The first listener of a mix that has just begun has nothing to catch up with: it starts at the beginning
        // of the mix.
        let mut position = (state.total == 0).then_some(0.0);
        if !state.recent.is_empty() {
            let joined: Vec<u8> = state.recent.iter().flat_map(|chunk| chunk.iter().copied()).collect();
            let data = &joined[joined.len().saturating_sub(self.preroll)..];
            let sent = &data[(self.config.frame_start)(data)..];
            client.queue.lock().expect("not poisoned").push_back(Bytes::copy_from_slice(sent));
            position = self.position_in(&state, state.total - sent.len());
        }
        state.clients.push(Arc::clone(&client));
        drop(state);
        self.first_listener.notify_waiters();
        (Listener { client, hub: Arc::clone(self) }, position)
    }

    /// Send a chunk of the stream to every listener, and keep it for those who join.
    pub fn publish(&self, chunk: Bytes) {
        let mut state = self.state();
        state.recent_size += chunk.len();
        state.total += chunk.len();
        state.recent.push_back(chunk.clone());
        let mark = (state.total, state.fed);
        state.marks.push_back(mark);
        while state.recent.len() > 1 && state.recent_size - state.recent[0].len() >= self.preroll {
            let gone = state.recent.pop_front().expect("there are two");
            state.recent_size -= gone.len();
        }
        while state.marks.len() > state.recent.len() + 1 {
            state.marks.pop_front();
        }
        for client in &state.clients {
            // A listener that can't keep up loses its oldest audio.
            let mut queue = client.queue.lock().expect("not poisoned");
            if queue.len() >= CLIENT_QUEUE_SIZE {
                queue.pop_front();
            }
            queue.push_back(chunk.clone());
            drop(queue);
            client.ready.notify_one();
        }
    }
}
