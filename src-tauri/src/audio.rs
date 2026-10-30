//! The native output is shared by the radio stream and the DJ console. A replacement source is prepared before the
//! audio callback switches to it; neither mode change closes the device or starts a track from the beginning.

mod output;

use crate::dj::Dj;
use output::{Input, Output, Switch};
use radio_core::radio::{PlaybackInfo, TrackInfo};
use rodio::{DeviceSinkBuilder, Player};
use serde::Serialize;
use std::{
    io::Read,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, sync_channel},
    },
    time::{Duration, Instant},
};
use symphonia::core::{
    codecs::audio::AudioDecoderOptions,
    errors::Error as SymphoniaError,
    formats::{FormatOptions, TrackType, probe::Hint},
    io::{MediaSourceStream, ReadOnlySource},
    meta::MetadataOptions,
};
use tauri::{AppHandle, Emitter, Manager};

enum Chunk {
    Start(f64),
    Pcm(Vec<f32>),
}
const CONNECT_PATIENCE: Duration = Duration::from_secs(30);
const QUEUE_CHUNKS: usize = 80;
const RECONNECT_TRIES: u32 = 12;
const RECONNECT_PAUSE: Duration = Duration::from_millis(1000);
const STABLE_FOR: Duration = Duration::from_secs(20);
const PREBUFFER_CHUNKS: usize = 12;
const STOPPED: u8 = 0;
const LOADING: u8 = 1;
const PLAYING: u8 = 2;
const ERROR: u8 = 3;

type Ready = Result<(u32, u16, f64), String>;

#[derive(Default)]
struct Shared {
    state: AtomicU8,
    samples: AtomicU64,
    stream_start: AtomicU64,
    stream_epoch: AtomicU64,
    has_stream: AtomicBool,
    dj: AtomicBool,
    paused: AtomicBool,
    underruns: AtomicU64,
    volume: AtomicU32,
    message: Mutex<String>,
    eq_gains: Mutex<Vec<f32>>,
    eq_version: AtomicU32,
    connection: Mutex<String>,
    playback: Mutex<Option<NowPlaying>>,
    generation: AtomicU64,
}

#[derive(Clone, Serialize)]
struct NowPlaying {
    #[serde(flatten)]
    track: TrackInfo,
    elapsed_seconds: f64,
    tempo: f64,
}

#[derive(Clone, Serialize)]
pub struct Status {
    state: &'static str,
    mode: &'static str,
    position: f64,
    mix_position: Option<f64>,
    stream_epoch: u64,
    paused: bool,
    underruns: u64,
    message: String,
    connection: String,
    volume: f32,
    track: Option<NowPlaying>,
}

struct Session {
    stop: Sender<()>,
    source: Sender<Switch>,
}

#[derive(Default)]
pub struct Audio {
    shared: Arc<Shared>,
    session: Mutex<Option<Session>>,
    reader: Mutex<Option<Arc<AtomicBool>>>,
    last_url: Mutex<Option<String>>,
}

pub(crate) struct Prepared {
    chunks: Option<Receiver<Chunk>>,
    ready: Receiver<Ready>,
    connection: String,
    stop: Arc<AtomicBool>,
    attached: bool,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if !self.attached {
            self.stop.store(true, Ordering::SeqCst);
        }
    }
}

impl Prepared {
    fn wait(mut self) -> Result<Input, String> {
        let (rate, channels, start) = self
            .ready
            .recv_timeout(Duration::from_secs(245))
            .map_err(|_| "the radio did not prepare audio in time".to_owned())??;
        if rate != 44_100 || channels != 2 {
            return Err("the radio must send stereo at 44.1 kHz".into());
        }
        if self.stop.load(Ordering::SeqCst) {
            return Err("stopped".into());
        }
        self.attached = true;
        Ok(Input::new(
            self.chunks.take().unwrap(),
            start,
            self.connection.clone(),
            self.stop.clone(),
        ))
    }
}

fn state_name(state: u8) -> &'static str {
    match state {
        LOADING => "loading",
        PLAYING => "playing",
        ERROR => "error",
        _ => "stopped",
    }
}

fn status(shared: &Shared) -> Status {
    Status {
        state: state_name(shared.state.load(Ordering::SeqCst)),
        mode: if shared.dj.load(Ordering::SeqCst) {
            "dj"
        } else {
            "radio"
        },
        position: shared.samples.load(Ordering::SeqCst) as f64 / 88_200.0,
        mix_position: shared.has_stream.load(Ordering::SeqCst).then(|| {
            f64::from_bits(shared.stream_start.load(Ordering::SeqCst))
                + shared.samples.load(Ordering::SeqCst) as f64 / 88_200.0
        }),
        stream_epoch: shared.stream_epoch.load(Ordering::SeqCst),
        paused: shared.paused.load(Ordering::SeqCst),
        underruns: shared.underruns.load(Ordering::SeqCst),
        message: shared.message.lock().unwrap().clone(),
        connection: shared.connection.lock().unwrap().clone(),
        volume: f32::from_bits(shared.volume.load(Ordering::SeqCst)),
        track: shared.playback.lock().unwrap().clone(),
    }
}

fn set_state(app: &AppHandle, shared: &Shared, state: u8, message: &str) {
    shared.state.store(state, Ordering::SeqCst);
    *shared.message.lock().unwrap() = message.to_owned();
    let _ = app.emit("player", status(shared));
}

fn open_sink() -> Result<rodio::MixerDeviceSink, rodio::DeviceSinkError> {
    DeviceSinkBuilder::from_default_device()
        .and_then(|b| {
            b.with_buffer_size(rodio::cpal::BufferSize::Default)
                .open_stream()
        })
        .or_else(|_| DeviceSinkBuilder::open_default_sink())
}

fn connection(url: &str) -> String {
    url.split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
        .split('&')
        .find_map(|p| p.strip_prefix("l="))
        .unwrap_or("")
        .to_owned()
}

impl Audio {
    fn ensure_output(&self, dj: &Dj) -> Result<Sender<Switch>, String> {
        let mut session = self.session.lock().unwrap();
        if let Some(session) = &*session {
            return Ok(session.source.clone());
        }
        let (stop, stopped) = mpsc::channel();
        let (source, commands) = mpsc::channel();
        let (ready, opened) = mpsc::channel();
        let shared = self.shared.clone();
        let console = dj.source();
        std::thread::spawn(move || {
            let sink = match open_sink() {
                Ok(sink) => sink,
                Err(e) => {
                    let _ = ready.send(Err(format!("no audio output: {e}")));
                    return;
                }
            };
            let player = Player::connect_new(sink.mixer());
            player.append(Output::new(shared, commands, console));
            let _ = ready.send(Ok(()));
            while let Err(RecvTimeoutError::Timeout) =
                stopped.recv_timeout(Duration::from_millis(250))
            {}
            player.stop();
        });
        opened
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "the audio output did not open".to_owned())??;
        *session = Some(Session {
            stop,
            source: source.clone(),
        });
        Ok(source)
    }

    pub fn start(
        &self,
        app: &AppHandle,
        dj: &Dj,
        url: String,
        volume: f32,
    ) -> Result<String, String> {
        self.set_volume(volume);
        if dj.is_running() {
            if dj.return_point().is_none() {
                return Err("no track is loaded in the console".into());
            }
            dj.play();
            self.shared.paused.store(false, Ordering::SeqCst);
            set_state(app, &self.shared, PLAYING, "");
            return Ok(self.shared.connection.lock().unwrap().clone());
        }
        if self.session.lock().unwrap().is_some()
            && self.shared.paused.swap(false, Ordering::SeqCst)
        {
            set_state(app, &self.shared, PLAYING, "");
            return Ok(self.shared.connection.lock().unwrap().clone());
        }
        let source = self.ensure_output(dj)?;
        self.shared.underruns.store(0, Ordering::SeqCst);
        set_state(app, &self.shared, LOADING, "");
        let token = connection(&url);
        *self.last_url.lock().unwrap() = Some(url.clone());
        let prepared = self.prepare(url);
        let generation = self.shared.generation.load(Ordering::SeqCst);
        let (app, shared) = (app.clone(), self.shared.clone());
        std::thread::spawn(move || {
            let result = prepared.wait().and_then(|input| {
                if shared.generation.load(Ordering::SeqCst) != generation
                    || shared.dj.load(Ordering::SeqCst)
                {
                    return Err("stopped".to_owned());
                }
                let (ack, applied) = mpsc::channel();
                source
                    .send(Switch::Radio {
                        input,
                        align: None,
                        paused: false,
                        ack,
                    })
                    .map_err(|_| "the audio output ended".to_owned())?;
                applied
                    .recv_timeout(Duration::from_secs(20))
                    .map_err(|_| "the audio output did not take the stream".to_owned())?
            });
            if shared.generation.load(Ordering::SeqCst) == generation
                && !shared.dj.load(Ordering::SeqCst)
            {
                match result {
                    Ok(()) => set_state(&app, &shared, PLAYING, ""),
                    Err(e) if e != "stopped" => set_state(&app, &shared, ERROR, &e),
                    _ => {}
                }
            }
        });
        Ok(token)
    }

    pub(crate) fn prepare(&self, url: String) -> Prepared {
        if let Some(old) = self.reader.lock().unwrap().take() {
            old.store(true, Ordering::SeqCst);
        }
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        let stop = Arc::new(AtomicBool::new(false));
        *self.reader.lock().unwrap() = Some(stop.clone());
        let (chunks, receiver) = sync_channel(QUEUE_CHUNKS);
        let (ready, waiting) = mpsc::channel();
        let token = connection(&url);
        let stopping = stop.clone();
        std::thread::spawn(move || {
            let mut tries = 0;
            loop {
                let started = Instant::now();
                let mut played = false;
                let Err(error) = read_stream(&url, &chunks, &ready, &stopping, &mut played) else {
                    return;
                };
                if stopping.load(Ordering::SeqCst) {
                    return;
                }
                if started.elapsed() >= STABLE_FOR {
                    tries = 0;
                }
                if !played && tries == 0 || tries >= RECONNECT_TRIES {
                    let _ = ready.send(Err(error));
                    return;
                }
                tries += 1;
                std::thread::sleep(RECONNECT_PAUSE * tries.min(5));
            }
        });
        Prepared {
            chunks: Some(receiver),
            ready: waiting,
            connection: token,
            stop,
            attached: false,
        }
    }

    pub(crate) fn enter_dj(
        &self,
        app: &AppHandle,
        dj: &Dj,
        mapping: PlaybackInfo,
        playing: bool,
    ) -> Result<(), String> {
        let source = self.ensure_output(dj)?;
        let (ack, applied) = mpsc::channel();
        source
            .send(Switch::Dj {
                mapping,
                playing,
                ack,
            })
            .map_err(|_| "the audio output ended".to_owned())?;
        applied
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "the DJ handoff timed out".to_owned())??;
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        self.announce(app);
        Ok(())
    }

    pub(crate) fn return_to_radio(
        &self,
        app: &AppHandle,
        dj: &Dj,
        prepared: Prepared,
        mapping: PlaybackInfo,
        paused: bool,
    ) -> Result<(), String> {
        let source = self.ensure_output(dj)?;
        let input = prepared.wait()?;
        let (ack, applied) = mpsc::channel();
        source
            .send(Switch::Radio {
                input,
                align: Some(mapping),
                paused,
                ack,
            })
            .map_err(|_| "the audio output ended".to_owned())?;
        applied
            .recv_timeout(Duration::from_secs(20))
            .map_err(|_| "the radio handoff timed out".to_owned())??;
        self.announce(app);
        Ok(())
    }

    pub(crate) fn stream_url(&self, base: &str) -> String {
        let flac = self
            .last_url
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|u| u.split('?').next().unwrap_or("").ends_with(".flac"));
        let url = format!(
            "{base}/stream{}?l={}",
            if flac { ".flac" } else { "" },
            uuid::Uuid::new_v4()
        );
        *self.last_url.lock().unwrap() = Some(url.clone());
        url
    }

    pub(crate) fn mix_position(&self) -> Option<f64> {
        self.shared.has_stream.load(Ordering::SeqCst).then(|| {
            f64::from_bits(self.shared.stream_start.load(Ordering::SeqCst))
                + self.shared.samples.load(Ordering::SeqCst) as f64 / 88_200.0
        })
    }
    pub(crate) fn epoch(&self) -> u64 {
        self.shared.generation.load(Ordering::SeqCst)
    }
    pub(crate) fn playing(&self) -> bool {
        self.shared.state.load(Ordering::SeqCst) == PLAYING
    }
    pub(crate) fn volume(&self) -> f32 {
        f32::from_bits(self.shared.volume.load(Ordering::SeqCst))
    }
    pub(crate) fn announce(&self, app: &AppHandle) {
        let _ = app.emit("player", self.status());
    }

    pub fn stop(&self, app: &AppHandle) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
        if let Some(reader) = self.reader.lock().unwrap().take() {
            reader.store(true, Ordering::SeqCst);
        }
        if let Some(session) = self.session.lock().unwrap().take() {
            let _ = session.stop.send(());
        }
        app.state::<Dj>().stop();
        self.shared.dj.store(false, Ordering::SeqCst);
        self.shared.paused.store(false, Ordering::SeqCst);
        *self.shared.playback.lock().unwrap() = None;
        set_state(app, &self.shared, STOPPED, "");
    }
    pub(crate) fn console_state(&self, app: &AppHandle, dj: &Dj) {
        let playing = dj.return_point().is_some_and(|p| p.playing);
        self.shared.paused.store(!playing, Ordering::SeqCst);
        set_state(
            app,
            &self.shared,
            if playing { PLAYING } else { STOPPED },
            "",
        );
    }

    pub fn set_equalizer(&self, gains: Vec<f32>) {
        *self.shared.eq_gains.lock().unwrap() = gains;
        self.shared.eq_version.fetch_add(1, Ordering::SeqCst);
    }
    pub fn set_volume(&self, volume: f32) {
        self.shared
            .volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::SeqCst);
    }
    pub fn status(&self) -> Status {
        status(&self.shared)
    }
}

struct SyncReader<R>(Mutex<R>);
impl<R: Read> Read for SyncReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.get_mut().unwrap().read(buf)
    }
}

fn read_stream(
    url: &str,
    chunks: &SyncSender<Chunk>,
    ready: &Sender<Ready>,
    stop: &AtomicBool,
    played: &mut bool,
) -> Result<(), String> {
    let fail = |message: String| {
        let _ = ready.send(Err(message.clone()));
        Err(message)
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_recv_response(Some(Duration::from_secs(240)))
        .build()
        .into();
    let deadline = Instant::now() + CONNECT_PATIENCE;
    let response = loop {
        match agent.get(url).call() {
            Ok(response) => break response,
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
    let start = response
        .headers()
        .get("x-stream-position")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(0.0);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let reader = SyncReader(Mutex::new(response.into_body().into_reader()));
    let mut hint = Hint::new();
    match content_type.as_str() {
        "audio/flac" | "audio/x-flac" => hint.with_extension("flac"),
        "audio/ogg" => hint.with_extension("ogg"),
        _ => hint.with_extension("mp3"),
    };
    let stream = MediaSourceStream::new(Box::new(ReadOnlySource::new(reader)), Default::default());
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            stream,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| format!("the stream is not understood: {e}"))?;
    let Some(track) = format.default_track(TrackType::Audio) else {
        return fail("no audio in the stream".into());
    };
    let track_id = track.id;
    let Some(params) = track.codec_params.as_ref().and_then(|p| p.audio()) else {
        return fail("no audio in the stream".into());
    };
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| format!("cannot decode the stream: {e}"))?;
    if chunks.send(Chunk::Start(start)).is_err() {
        return Ok(());
    }
    let mut said = false;
    let mut kept = 0;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => return Err("the stream has ended".into()),
            Err(e) => return Err(format!("the stream broke: {e}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_) | SymphoniaError::IoError(_)) => continue,
            Err(e) => return Err(format!("cannot decode the stream: {e}")),
        };
        let spec = decoded.spec();
        let mut samples = Vec::new();
        decoded.copy_to_vec_interleaved(&mut samples);
        if samples.is_empty() {
            continue;
        }
        if !said {
            kept += 1;
            if kept >= PREBUFFER_CHUNKS {
                said = true;
                *played = true;
                let _ = ready.send(Ok((spec.rate(), spec.channels().count() as u16, start)));
            }
        }
        if chunks.send(Chunk::Pcm(samples)).is_err() {
            return Ok(());
        }
    }
}
