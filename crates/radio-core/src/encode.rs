//! Mixing, encoding and publishing in real time while somebody listens. Tasks 3.4.1 to 3.4.3 of
//! docs/rust-port-radio.md, ported from `dj.py`: `_hold`, `_note_position`, `_encode_loop`, `encode_forever`.
//!
//! The mix is cut into pieces; for each one the loop waits while the mix is paused or nobody listens, tells the streams
//! and the spectrum where the mix is, feeds the encoders and sleeps when it has run ahead of the listeners.

use crate::{
    clock::Clock,
    mix::{CHANNELS, SAMPLE_RATE},
    pipeline::{Closed, DeckSource, MixError, Piece, PieceSink, SharedNoise, mix},
    spectrum::{SpectrumAnalyzer, SpectrumFeed},
    stream::{
        broadcaster::Broadcaster,
        encoder::{Encoder, Format},
    },
};
use async_trait::async_trait;
use std::{io, sync::Arc, time::Duration};
use tokio::sync::Mutex;

/// How far ahead of the listeners the mix may run.
pub const PACING_LEAD_SECONDS: f64 = 2.0;
/// Pause before the mix starts again after a failure.
pub const RESTART_SECONDS: f64 = 5.0;

/// The streams that are served: the MP3 one, and the lossless ones if they are on.
#[derive(Clone)]
pub struct Streams {
    pub mp3: Arc<Broadcaster>,
    pub flac: Option<Arc<Broadcaster>>,
    pub ogg: Option<Arc<Broadcaster>>,
}

impl Streams {
    /// Each stream with its format, the MP3 one first.
    pub fn all(&self) -> Vec<(Format, Arc<Broadcaster>)> {
        let mut streams = vec![(Format::Mp3, Arc::clone(&self.mp3))];
        streams.extend(self.flac.iter().map(|hub| (Format::Flac, Arc::clone(hub))));
        streams.extend(self.ogg.iter().map(|hub| (Format::Ogg, Arc::clone(hub))));
        streams
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// How far ahead of the listeners the mix may run, in seconds.
    pub pacing_lead: f64,
    /// How long to wait before the mix starts again after a failure.
    pub restart: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            pacing_lead: PACING_LEAD_SECONDS,
            restart: Duration::from_secs_f64(RESTART_SECONDS),
        }
    }
}

/// What ended a mix.
#[derive(Debug)]
pub enum EncodeError {
    /// The mix could not go on.
    Mix(MixError),
    /// An encoder could not be started, or taken more of the mix.
    Encoder(io::Error),
}

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mix(error) => write!(f, "the mix failed: {error}"),
            Self::Encoder(error) => write!(f, "an encoder failed: {error}"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// Wait while the mix is paused or nobody listens; say if it waited.
pub async fn hold(radio: &dyn DeckSource, hubs: &[Arc<Broadcaster>]) -> bool {
    let mut held = false;
    while radio.user_paused() || !hubs.iter().any(|hub| hub.has_listeners()) {
        if !held {
            radio.hold(true);
            held = true;
        }
        if radio.user_paused() {
            radio.wait_resumed().await;
        } else {
            let others: Vec<&Broadcaster> = hubs[1..].iter().map(|hub| &**hub).collect();
            hubs[0].wait_for_a_listener(&others).await;
        }
    }
    if held {
        radio.hold(false);
    }
    held
}

/// What the loop keeps between two pieces.
struct State<E> {
    encoders: Vec<E>,
    analyzer: SpectrumAnalyzer,
    /// When the pacing began, and the seconds of the mix made since.
    started: f64,
    produced: f64,
    /// The error that made the sink stop taking pieces.
    failure: Option<io::Error>,
}

/// Takes the pieces of the mix and does what the Python loop does with each.
struct Sink<E> {
    radio: Arc<dyn DeckSource>,
    clock: Arc<dyn Clock>,
    hubs: Vec<Arc<Broadcaster>>,
    spectrum: Option<Arc<SpectrumFeed>>,
    options: Options,
    state: Mutex<State<E>>,
}

#[async_trait]
impl<E: Encoder + Send> PieceSink for Sink<E> {
    async fn send(&self, piece: Piece) -> Result<(), Closed> {
        if hold(&*self.radio, &self.hubs).await {
            let mut state = self.state.lock().await;
            state.started = self.clock.monotonic();
            state.produced = 0.0;
        }
        self.hubs[0].set_title(&piece.title);
        let mut state = self.state.lock().await;
        // Before the encoders get it: what they write next is of this.
        let frames = state.analyzer.feed(&piece.pcm);
        for hub in &self.hubs {
            hub.set_fed(state.analyzer.position());
        }
        if let Some(spectrum) = &self.spectrum {
            spectrum.publish(frames);
        }
        for encoder in &mut state.encoders {
            if let Err(error) = encoder.write(&piece.pcm).await {
                state.failure = Some(error);
                return Err(Closed);
            }
        }
        state.produced += (piece.pcm.len() / CHANNELS) as f64 / f64::from(SAMPLE_RATE);
        let ahead =
            state.produced - (self.clock.monotonic() - state.started) - self.options.pacing_lead;
        drop(state);
        if ahead > 0.0 {
            tokio::time::sleep(Duration::from_secs_f64(ahead)).await;
        }
        Ok(())
    }
}

/// Mix, encode and publish in real time while somebody listens. Ends with the error that ended the mix.
#[allow(clippy::too_many_arguments)]
pub async fn encode_loop<E, F>(
    radio: Arc<dyn DeckSource>,
    clock: Arc<dyn Clock>,
    effects_level: f64,
    noise: SharedNoise,
    streams: &Streams,
    spectrum: Option<Arc<SpectrumFeed>>,
    start_encoder: &F,
    options: Options,
) -> Result<(), EncodeError>
where
    E: Encoder + Send,
    F: Fn(Format, Arc<Broadcaster>) -> io::Result<E> + Send + Sync,
{
    let all = streams.all();
    let hubs: Vec<Arc<Broadcaster>> = all.iter().map(|(_, hub)| Arc::clone(hub)).collect();
    // The positions in the mix start over with it.
    for hub in &hubs {
        hub.reset_position();
    }
    if let Some(spectrum) = &spectrum {
        spectrum.reset();
    }
    let mut encoders = Vec::new();
    for (format, hub) in &all {
        match start_encoder(*format, Arc::clone(hub)) {
            Ok(encoder) => encoders.push(encoder),
            Err(error) => {
                for started in encoders {
                    started.stop().await;
                }
                return Err(EncodeError::Encoder(error));
            }
        }
    }
    let sink = Sink {
        radio: Arc::clone(&radio),
        clock: Arc::clone(&clock),
        hubs,
        spectrum,
        options,
        state: Mutex::new(State {
            encoders,
            analyzer: SpectrumAnalyzer::new(),
            started: clock.monotonic(),
            produced: 0.0,
            failure: None,
        }),
    };
    let ended = mix(radio, clock, effects_level, noise, &sink).await;
    let State {
        encoders, failure, ..
    } = sink.state.into_inner();
    for encoder in encoders {
        encoder.stop().await;
    }
    match (failure, ended) {
        (Some(error), _) => Err(EncodeError::Encoder(error)),
        (None, Err(MixError::Closed)) => Ok(()),
        (None, Err(error)) => Err(EncodeError::Mix(error)),
        (None, Ok(())) => Ok(()),
    }
}

/// Keep the mix going: a failure ends only the mix it happened in. Returns only if the mix ends without one.
#[allow(clippy::too_many_arguments)]
pub async fn encode_forever<E, F>(
    radio: Arc<dyn DeckSource>,
    clock: Arc<dyn Clock>,
    effects_level: f64,
    noise: SharedNoise,
    streams: Streams,
    spectrum: Option<Arc<SpectrumFeed>>,
    start_encoder: F,
    options: Options,
) where
    E: Encoder + Send,
    F: Fn(Format, Arc<Broadcaster>) -> io::Result<E> + Send + Sync,
{
    loop {
        let ended = encode_loop(
            Arc::clone(&radio),
            Arc::clone(&clock),
            effects_level,
            Arc::clone(&noise),
            &streams,
            spectrum.clone(),
            &start_encoder,
            options,
        )
        .await;
        match ended {
            Ok(()) => return,
            Err(error) => {
                tracing::error!(
                    "{error}, starting it again in {} s",
                    options.restart.as_secs_f64()
                );
                tokio::time::sleep(options.restart).await;
            }
        }
    }
}
