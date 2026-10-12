//! The native player: the stream of the radio is read, decoded and played here, by the system's
//! audio output (CoreAudio on macOS, ALSA on Linux), and not by the `<audio>` of the webview.
//!
//! Three threads work for one listening: one reads and decodes the stream (HTTP, `symphonia`) and
//! hands out pieces of audio, one owns the output device and the player (`rodio`; the device may
//! not leave its thread), and the audio callback of the system pulls the pieces. The page asks
//! for `player_start`, `player_stop` and `player_volume`, hears the state as the event "player",
//! and reads the position of what is *played* with `player_status` (the spectrum follows it).
//!
//! When something is wrong before the first sound (no device, no stream, a format that is not
//! understood), the state is "error" and the page falls back to the `<audio>` of the webview.

use std::io::Read;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rodio::{ChannelCount, DeviceSinkBuilder, Player, SampleRate, Source};
use serde::Serialize;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSourceStream, ReadOnlySource};
use symphonia::core::meta::MetadataOptions;
use tauri::{AppHandle, Emitter};

/// A piece of audio: interleaved samples of the pieces the decoder makes (~25 ms of an MP3 frame).
type Chunk = Vec<f32>;
/// How many pieces wait for the output: ~2 s. The stream is live, so the reader is held back
/// (the network does the waiting) only a little.
/// How long the reader tries to reach the radio (it may be starting).
const CONNECT_PATIENCE: Duration = Duration::from_secs(30);
const QUEUE_CHUNKS: usize = 80;
/// How many pieces are kept before the sound starts: ~0.3 s against a gap at the very start.
const PREBUFFER_CHUNKS: usize = 12;

const STOPPED: u8 = 0;
const LOADING: u8 = 1;
const PLAYING: u8 = 2;
const ERROR: u8 = 3;

#[derive(Default)]
struct Shared {
    state: AtomicU8,
    /// The samples that were played (the silence put in a gap is not counted).
    samples: AtomicU64,
    rate: AtomicU32,
    channels: AtomicU32,
    underruns: AtomicU64,
    /// The volume as the bits of an f32.
    volume: AtomicU32,
    message: Mutex<String>,
}

/// What `player_status` says.
#[derive(Serialize)]
pub struct Status {
    state: &'static str,
    /// Seconds of the stream that were played.
    position: f64,
    /// How many times the output waited for audio that had not come yet.
    underruns: u64,
    message: String,
}

enum Command {
    Stop,
    Volume(f32),
}

struct Session {
    commands: Sender<Command>,
    stop: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct Audio {
    shared: Arc<Shared>,
    session: Mutex<Option<Session>>,
}

fn state_name(state: u8) -> &'static str {
    match state {
        LOADING => "loading",
        PLAYING => "playing",
        ERROR => "error",
        _ => "stopped",
    }
}

#[derive(Serialize, Clone)]
struct StateEvent {
    state: &'static str,
    message: String,
}

fn set_state(app: &AppHandle, shared: &Shared, state: u8, message: &str) {
    shared.state.store(state, Ordering::SeqCst);
    *shared.message.lock().unwrap() = message.to_string();
    let _ = app.emit("player", StateEvent { state: state_name(state), message: message.to_string() });
}

impl Audio {
    pub fn start(&self, app: &AppHandle, url: String, volume: f32) {
        self.stop(app);
        let shared = self.shared.clone();
        shared.samples.store(0, Ordering::SeqCst);
        shared.underruns.store(0, Ordering::SeqCst);
        shared.volume.store(volume.to_bits(), Ordering::SeqCst);
        set_state(app, &shared, LOADING, "");

        let (commands, command_rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        *self.session.lock().unwrap() = Some(Session { commands, stop: stop.clone() });

        let (chunk_tx, chunk_rx) = sync_channel::<Chunk>(QUEUE_CHUNKS);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(u32, u16), String>>();

        // The reader of the stream.
        {
            let (shared, stop, app) = (shared.clone(), stop.clone(), app.clone());
            std::thread::spawn(move || {
                if let Err(message) = read_stream(&url, &chunk_tx, &ready_tx, &stop) {
                    if !stop.load(Ordering::SeqCst) {
                        // Before the first sound the page falls back; later it only says so.
                        set_state(&app, &shared, ERROR, &message);
                    }
                }
            });
        }

        // The owner of the output device.
        let app = app.clone();
        std::thread::spawn(move || {
            let (rate, channels) = match ready_rx.recv() {
                Ok(Ok(format)) => format,
                _ => return, // the reader has said what is wrong
            };
            shared.rate.store(rate, Ordering::SeqCst);
            shared.channels.store(channels as u32, Ordering::SeqCst);
            let (Some(rate), Some(channels)) = (NonZero::new(rate), NonZero::new(channels)) else {
                set_state(&app, &shared, ERROR, "the stream has no audio format");
                return;
            };

            let sink = match DeviceSinkBuilder::open_default_sink() {
                Ok(sink) => sink,
                Err(error) => {
                    stop.store(true, Ordering::SeqCst);
                    set_state(&app, &shared, ERROR, &format!("no audio output: {error}"));
                    return;
                }
            };
            let player = Player::connect_new(sink.mixer());
            player.set_volume(f32::from_bits(shared.volume.load(Ordering::SeqCst)));
            player.append(StreamSource {
                chunks: chunk_rx,
                current: Vec::new(),
                at: 0,
                channels,
                rate,
                shared: shared.clone(),
                gap: false,
            });
            set_state(&app, &shared, PLAYING, "");

            loop {
                match command_rx.recv_timeout(Duration::from_millis(250)) {
                    Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                    Ok(Command::Volume(volume)) => player.set_volume(volume),
                    Err(RecvTimeoutError::Timeout) => {
                        // The stream ended (the radio went away): nothing is left to play.
                        if player.empty() && shared.state.load(Ordering::SeqCst) == PLAYING {
                            set_state(&app, &shared, ERROR, "the stream has ended");
                            break;
                        }
                    }
                }
            }
            stop.store(true, Ordering::SeqCst);
            player.stop();
            drop(player);
            drop(sink);
        });
    }

    pub fn stop(&self, app: &AppHandle) {
        if let Some(session) = self.session.lock().unwrap().take() {
            session.stop.store(true, Ordering::SeqCst);
            let _ = session.commands.send(Command::Stop);
            set_state(app, &self.shared, STOPPED, "");
        }
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared.volume.store(volume.to_bits(), Ordering::SeqCst);
        if let Some(session) = self.session.lock().unwrap().as_ref() {
            let _ = session.commands.send(Command::Volume(volume));
        }
    }

    pub fn status(&self) -> Status {
        let shared = &self.shared;
        let rate = shared.rate.load(Ordering::SeqCst).max(1) as f64;
        let channels = shared.channels.load(Ordering::SeqCst).max(1) as f64;
        Status {
            state: state_name(shared.state.load(Ordering::SeqCst)),
            position: shared.samples.load(Ordering::SeqCst) as f64 / (rate * channels),
            underruns: shared.underruns.load(Ordering::SeqCst),
            message: shared.message.lock().unwrap().clone(),
        }
    }
}

/// The audio the output pulls: the pieces the reader made, one sample at a time. It must not
/// wait for the network (that would stop the sound), so a gap is filled with silence.
struct StreamSource {
    chunks: Receiver<Chunk>,
    current: Chunk,
    at: usize,
    channels: ChannelCount,
    rate: SampleRate,
    shared: Arc<Shared>,
    gap: bool,
}

impl Iterator for StreamSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.at >= self.current.len() {
            match self.chunks.try_recv() {
                Ok(chunk) => {
                    self.current = chunk;
                    self.at = 0;
                    self.gap = false;
                }
                Err(TryRecvError::Empty) => {
                    if !self.gap {
                        self.gap = true;
                        self.shared.underruns.fetch_add(1, Ordering::Relaxed);
                    }
                    return Some(0.0);
                }
                Err(TryRecvError::Disconnected) => return None,
            }
        }
        let sample = self.current[self.at];
        self.at += 1;
        self.shared.samples.fetch_add(1, Ordering::Relaxed);
        Some(sample)
    }
}

impl Source for StreamSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        self.channels
    }
    fn sample_rate(&self) -> SampleRate {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// `symphonia` wants a reader it can share between threads.
struct SyncReader<R>(Mutex<R>);

impl<R: Read> Read for SyncReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.get_mut().unwrap().read(buf)
    }
}

/// Reads the stream and decodes it; the pieces go to `chunks`. After the first ones are in it
/// says (`ready`) what the audio is: the sample rate and the channels.
fn read_stream(
    url: &str,
    chunks: &SyncSender<Chunk>,
    ready: &Sender<Result<(u32, u16), String>>,
    stop: &AtomicBool,
) -> Result<(), String> {
    let fail = |message: String| {
        let _ = ready.send(Err(message.clone()));
        Err(message)
    };

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(5)))
        // The radio answers only when it has a track to play: right after its start that takes a while.
        .timeout_recv_response(Some(Duration::from_secs(40)))
        .build()
        .into();
    // The service of a freshly started app is not listening yet (it takes a few seconds): try again
    // for a while instead of giving up (and the page falling back to the browser) at the first refusal.
    let deadline = Instant::now() + CONNECT_PATIENCE;
    let response = loop {
        match agent.get(url).call() {
            Ok(response) => break response,
            // A wrong address or a refusal will not mend itself.
            Err(ureq::Error::StatusCode(code)) if code < 500 => {
                return fail(format!("the radio answered {code}"));
            }
            Err(error) => {
                if stop.load(Ordering::SeqCst) {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return fail(format!("cannot connect to the radio: {error}"));
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    };
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let reader = SyncReader(Mutex::new(response.into_body().into_reader()));

    let mut hint = Hint::new();
    match content_type.as_str() {
        "audio/flac" | "audio/x-flac" => hint.with_extension("flac"),
        "audio/ogg" => hint.with_extension("ogg"),
        _ => hint.with_extension("mp3"),
    };
    let stream = MediaSourceStream::new(Box::new(ReadOnlySource::new(reader)), Default::default());
    let mut format = match symphonia::default::get_probe().probe(
        &hint,
        stream,
        FormatOptions::default(),
        MetadataOptions::default(),
    ) {
        Ok(format) => format,
        Err(error) => return fail(format!("the stream is not understood: {error}")),
    };
    let Some(track) = format.default_track(TrackType::Audio) else {
        return fail("no audio in the stream".into());
    };
    let track_id = track.id;
    let Some(params) = track.codec_params.as_ref().and_then(|params| params.audio()) else {
        return fail("no audio in the stream".into());
    };
    let mut decoder = match symphonia::default::get_codecs().make_audio_decoder(params, &AudioDecoderOptions::default()) {
        Ok(decoder) => decoder,
        Err(error) => return fail(format!("cannot decode the stream: {error}")),
    };

    let mut said = false;
    let mut kept = 0;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => return Err("the stream has ended".into()),
            Err(error) => return Err(format!("the stream broke: {error}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // A damaged packet: skip it, the sound goes on.
            Err(SymphoniaError::DecodeError(_) | SymphoniaError::IoError(_)) => continue,
            Err(error) => return Err(format!("cannot decode the stream: {error}")),
        };
        let spec = decoded.spec();
        let mut samples: Chunk = Vec::new();
        decoded.copy_to_vec_interleaved(&mut samples);
        if samples.is_empty() {
            continue;
        }
        if !said {
            kept += 1;
            if kept >= PREBUFFER_CHUNKS {
                said = true;
                let _ = ready.send(Ok((spec.rate(), spec.channels().count() as u16)));
            }
        }
        // Waits when the queue is full; goes away when the player is gone.
        if chunks.send(samples).is_err() {
            return Ok(());
        }
    }
}
