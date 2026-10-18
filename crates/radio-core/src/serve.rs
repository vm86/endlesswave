//! Serve the stream and the API, then mix until told to stop (`dj.serve` of the Python radio). Task 3.4.4 of
//! docs/rust-port-radio.md.

use crate::{
    api::{Api, BackendSet, serve as serve_api},
    backend::MusicBackend,
    cache::TrackCache,
    clock::{Clock, SystemClock},
    encode::{Options, Streams, encode_forever},
    mix::effects::RandomNoise,
    pipeline::{DeckSource, SharedNoise},
    radio::{DEFAULT_POOL_SIZE, Radio, RadioConfig},
    spectrum::{SpectrumFeed, StartPositions},
    stream::{
        broadcaster::{Broadcaster, Config},
        encoder::{Format, FfmpegEncoder},
        native::{AnyEncoder, NativeEncoder},
    },
};
use std::{
    future::Future,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

/// How long to wait before looking for the first tracks again, when there were none.
pub const STARTUP_RETRY_SECONDS: u64 = 30;

/// Club effects levels by name.
pub fn effects_level(name: &str) -> Option<f64> {
    match name {
        "off" => Some(0.0),
        "light" => Some(0.6),
        "full" => Some(1.0),
        _ => None,
    }
}

pub struct ServeOptions {
    pub host: String,
    pub port: u16,
    pub station_id: String,
    pub pool_size: usize,
    /// One of the `effects_level` values.
    pub effects: f64,
    pub mood_arc: bool,
    /// Where the tracks that wait are kept across runs; used only with a cache. Without, they are in a directory that
    /// goes with the run.
    pub tracks_dir: Option<PathBuf>,
    pub ffmpeg: PathBuf,
    /// Encode with ffmpeg processes instead of the encoders built in.
    pub ffmpeg_encoders: bool,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_owned(),
            port: 8000,
            station_id: String::new(),
            pool_size: DEFAULT_POOL_SIZE,
            effects: 0.6,
            mood_arc: false,
            tracks_dir: None,
            ffmpeg: PathBuf::from("ffmpeg"),
            ffmpeg_encoders: false,
        }
    }
}

/// Fill the pool, and go on trying while no backend gives any tracks. The radio must stay up, and tell on its page and in
/// the log what is wrong, rather than stop and be started again by whoever runs it.
async fn first_tracks(radio: &Radio, retry: Duration) {
    loop {
        if let Err(error) = radio.fill(None).await {
            tracing::error!("Could not load the first tracks: {error}");
        }
        if !radio.pool().is_empty() {
            return;
        }
        tracing::warn!("No playable tracks yet, is a music backend reachable? Retry in {} s", retry.as_secs());
        tokio::time::sleep(retry).await;
    }
}

/// Serve the stream and the API, then mix until `shutdown` is done. The server is up before the app is authorized, so
/// that `/api/auth` can tell where to do it. `ready` gets the address once the server listens.
pub async fn serve(
    backend: Arc<dyn MusicBackend>,
    selectable: Option<Arc<dyn BackendSet>>,
    options: ServeOptions,
    cache: Option<TrackCache>,
    ready: Option<tokio::sync::oneshot::Sender<SocketAddr>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    serve_with(backend, selectable, options, cache, ready, shutdown, Duration::from_secs(STARTUP_RETRY_SECONDS)).await
}

pub async fn serve_with(
    backend: Arc<dyn MusicBackend>,
    selectable: Option<Arc<dyn BackendSet>>,
    options: ServeOptions,
    cache: Option<TrackCache>,
    ready: Option<tokio::sync::oneshot::Sender<SocketAddr>>,
    shutdown: impl Future<Output = ()> + Send + 'static,
    startup_retry: Duration,
) -> std::io::Result<()> {
    let keep = cache.is_some() && options.tracks_dir.is_some();
    let (workdir, temporary) = match (&options.tracks_dir, keep) {
        (Some(dir), true) => (dir.clone(), false),
        _ => (std::env::temp_dir().join(format!("yamusic-dj-{}", uuid::Uuid::new_v4())), true),
    };
    tokio::fs::create_dir_all(&workdir).await?;

    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    let radio = Radio::new(
        backend.clone(),
        workdir.clone(),
        RadioConfig {
            station_id: options.station_id.clone(),
            pool_size: options.pool_size,
            mood_arc: options.mood_arc,
            keep_playlist: keep,
            ffmpeg: options.ffmpeg.clone(),
        },
        cache,
        clock.clone(),
    );
    let streams = Streams {
        mp3: Broadcaster::new(Config::mp3()),
        flac: Some(Broadcaster::new(Config::flac())),
        ogg: Some(Broadcaster::new(Config::ogg())),
    };
    let spectrum = SpectrumFeed::new();
    let api = Api {
        radio: radio.clone(),
        backends: selectable,
        streams: streams.clone(),
        spectrum: Some(spectrum.clone()),
        starts: Arc::new(StartPositions::new()),
    };
    let listener = tokio::net::TcpListener::bind((options.host.as_str(), options.port)).await?;
    let address = listener.local_addr()?;
    tracing::info!("Streaming at http://{address}/stream (MP3), /stream.flac, /stream.ogg");
    let (stop_server, server_stopped) = tokio::sync::oneshot::channel::<()>();
    let mut server = tokio::spawn(serve_api(listener, api, async move {
        let _ = server_stopped.await;
    }));
    if let Some(ready) = ready {
        let _ = ready.send(address);
    }

    let work = {
        let radio = radio.clone();
        let backend = backend.clone();
        let ffmpeg = options.ffmpeg.clone();
        let ffmpeg_encoders = options.ffmpeg_encoders;
        async move {
            // Waiting for the user to authorize the app.
            backend.wait_authorized().await;
            // What the last run left: a failure here must not stop the radio.
            if let Err(error) = radio.restore_state().await {
                tracing::error!("Could not take up the picks of the last run: {error}");
            }
            radio.restore_playlist();
            if options.mood_arc && radio.mood_arc_saved().is_none() {
                if let Err(error) = radio.start_mood_arc().await {
                    tracing::error!("Could not start the mood arc: {error}");
                }
            }
            tracing::info!("Preparing {} tracks from {}...", options.pool_size, radio.station_id());
            first_tracks(&radio, startup_retry).await;
            let refill = tokio::spawn({
                let radio = radio.clone();
                async move { radio.keep_filled().await }
            });
            let noise: SharedNoise = Arc::new(Mutex::new(RandomNoise::from_entropy()));
            let mut refill = refill;
            let mix = encode_forever(
                radio.clone() as Arc<dyn DeckSource>,
                clock,
                options.effects,
                noise,
                streams,
                Some(spectrum),
                move |format: Format, hub: Arc<Broadcaster>| {
                    if ffmpeg_encoders { FfmpegEncoder::start(&ffmpeg, format, hub).map(AnyEncoder::Ffmpeg) } else { NativeEncoder::start(format, hub).map(AnyEncoder::Native) }
                },
                Options::default(),
            );
            // A task that ends (it panicked) leaves the radio half dead: end it all, whoever runs it starts it again.
            tokio::select! {
                () = mix => {}
                ended = &mut refill => tracing::error!("The refill of the pool ended: {ended:?}"),
            }
            refill.abort();
        }
    };
    tokio::select! {
        () = work => {}
        () = shutdown => tracing::info!("Shutting down..."),
        ended = &mut server => tracing::error!("The server ended: {ended:?}"),
    }
    // The mood is a setting of the account: leave it as it was.
    if let Err(error) = radio.restore_settings().await {
        tracing::error!("Could not put the station settings back: {error}");
    }
    // The streams never end by themselves: a listener that is still there would keep a graceful stop waiting for ever.
    let _ = stop_server.send(());
    if tokio::time::timeout(Duration::from_secs(2), &mut server).await.is_err() {
        server.abort();
    }
    if temporary {
        let _ = tokio::fs::remove_dir_all(&workdir).await;
    }
    Ok(())
}
