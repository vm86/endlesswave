//! The command line and the wiring of the daemon (`main.py` of the Python radio, without the fork into the background and
//! the systemd notification). Task 3.4.4 of docs/rust-port-radio.md.

use crate::{
    api::BackendSet,
    backend::{
        MusicBackend,
        local::LocalBackend,
        multi::{BackendSpec, MultiBackend},
        yandex::{self, YandexBackend, YandexOptions},
    },
    cache::TrackCache,
    clock::{Clock, SystemClock},
    radio::DEFAULT_POOL_SIZE,
    serve::{ServeOptions, effects_level, serve},
};
use clap::Parser;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

/// The ids of `backend_specs`, for the command line. (YouTube Music comes later.)
pub const BACKEND_IDS: [&str; 2] = ["yandex", "local"];
/// The cache setting that keeps the backends turned on (the older one, "backend", kept just the one).
pub const SETTING_BACKENDS: &str = "backends";
pub const SETTING_BACKEND: &str = "backend";
/// The folder of the local backend.
pub const SETTING_LOCAL_DIRECTORY: &str = "local_directory";

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// Make a path absolute (and `~` the home folder): the tracks are linked to a folder, and a relative path would give links
/// that point nowhere.
fn absolute(value: &str) -> PathBuf {
    let expanded = match value.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if value == "~" => home(),
        None => PathBuf::from(value),
    };
    std::fs::canonicalize(&expanded).unwrap_or_else(|_| {
        if expanded.is_absolute() {
            expanded
        } else {
            std::env::current_dir().unwrap_or_default().join(expanded)
        }
    })
}

#[derive(Parser, Debug, Clone)]
#[command(
    about = "DJ radio: take tracks from Yandex Music and/or a local folder, beat-match them into one endless mix and stream it as MP3, FLAC and Ogg over HTTP (needs ffmpeg)"
)]
pub struct Options {
    /// Enable debugging output
    #[arg(long)]
    pub debug: bool,
    /// Settings file with the Yandex OAuth token (Yandex backend)
    #[arg(long, default_value_os_t = crate::paths::config_file())]
    pub config: PathBuf,
    /// Turn a music backend on (repeat for several). The tracks of all that are on are mixed. The ones of the last run and
    /// the ones that are signed in are on too; the API turns them on and off
    #[arg(long, value_parser = clap::builder::PossibleValuesParser::new(BACKEND_IDS))]
    pub backend: Vec<String>,
    /// Folder with audio files for the local backend; it is turned on and the folder kept in the settings (the API can
    /// change it)
    #[arg(long, value_parser = |v: &str| Ok::<_, String>(absolute(v)))]
    pub local_dir: Option<PathBuf>,
    /// HTTP bind address of the stream
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,
    /// HTTP port of the stream (/stream)
    #[arg(long, default_value_t = 8000)]
    pub port: u16,
    /// Tracks to download and analyze before the radio starts
    #[arg(long, default_value_t = DEFAULT_POOL_SIZE)]
    pub tracks: usize,
    /// Station to take tracks from: ID=STATION for the backend ID (repeat for several), or just STATION for the first
    /// backend. Each backend has its own by default
    #[arg(long)]
    pub station: Vec<String>,
    /// Club effects on transitions: a riser and a boom on the drop, an echo-out when the beats can't be matched
    #[arg(long, default_value = "light", value_parser = ["off", "light", "full"])]
    pub effects: String,
    /// Switch the mood setting of the station along the energy arc of the set (calm, fun, active). The setting is kept on
    /// your Yandex account: it is changed while the radio runs and put back when it stops
    #[arg(long)]
    pub mood_arc: bool,
    /// Database of track analyses and play times (no audio is stored): a restart doesn't repeat tracks or analyze them
    /// again
    #[arg(long, default_value_os_t = crate::paths::cache_file())]
    pub cache: PathBuf,
    /// Where the tracks that are loaded and wait for their turn are kept, so that a restart plays them and doesn't
    /// download them again (default: next to the cache, as <cache name>-tracks)
    #[arg(long)]
    pub tracks_dir: Option<PathBuf>,
    /// Keep no tracks between runs: they are loaded again at a start
    #[arg(long)]
    pub no_keep_tracks: bool,
    /// Don't keep anything on disk between runs
    #[arg(long)]
    pub no_cache: bool,
    /// Encode the streams with ffmpeg processes instead of the encoders built into the program
    #[arg(long)]
    pub ffmpeg_encoders: bool,
    /// The ffmpeg to run
    #[arg(long, default_value = "ffmpeg")]
    pub ffmpeg: PathBuf,
}

type SharedCache = Arc<Mutex<TrackCache>>;

fn saved_directory(cache: Option<&SharedCache>) -> Option<PathBuf> {
    let saved = cache?
        .lock()
        .expect("not poisoned")
        .get_setting(SETTING_LOCAL_DIRECTORY);
    saved.and_then(|v| v.as_str().filter(|s| !s.is_empty()).map(PathBuf::from))
}

/// List the music services the radio can take its tracks from. The one place to change to play from another service: add
/// a `BackendSpec` that makes any `MusicBackend`.
pub fn backend_specs(
    options: &Options,
    cache: Option<&SharedCache>,
    clock: Arc<dyn Clock>,
) -> Vec<BackendSpec> {
    let directory = options.local_dir.clone().or_else(|| saved_directory(cache));
    let config = options.config.clone();
    let yandex_clock = clock.clone();
    let shown = directory.clone();
    let remember = cache.cloned();
    let ffmpeg = options.ffmpeg.clone();
    vec![
        BackendSpec::new("yandex", "Yandex Music", {
            let config = config.clone();
            move || {
                YandexBackend::new(
                    config.clone(),
                    YandexOptions::default(),
                    yandex_clock.clone(),
                )
                .expect("the settings file of the Yandex backend can be read")
                    as Arc<dyn MusicBackend>
            }
        })
        .configured(move || yandex::has_token(&config)),
        BackendSpec::new("local", "Local folder", {
            let directory = directory.clone();
            move || {
                let on_directory: Option<Box<dyn Fn(&str) + Send + Sync>> =
                    remember.clone().map(|cache| {
                        Box::new(move |chosen: &str| {
                            if let Err(error) = cache
                                .lock()
                                .expect("not poisoned")
                                .put_setting(SETTING_LOCAL_DIRECTORY, &json!(chosen))
                            {
                                tracing::error!("Could not keep the folder: {error}");
                            }
                        }) as Box<dyn Fn(&str) + Send + Sync>
                    });
                Arc::new(
                    LocalBackend::new(directory.clone(), on_directory, clock.clone())
                        .with_ffmpeg(ffmpeg.clone()),
                ) as Arc<dyn MusicBackend>
            }
        })
        .configured(move || shown.as_deref().is_some_and(Path::is_dir)),
    ]
}

/// The backends that were on in the last run, `None` if not known.
pub fn last_backends(cache: Option<&SharedCache>) -> Option<Vec<String>> {
    let cache = cache?.lock().expect("not poisoned");
    match cache.get_setting(SETTING_BACKENDS) {
        Some(Value::Array(ids)) => Some(
            ids.iter()
                .filter_map(|i| i.as_str().map(str::to_owned))
                .collect(),
        ),
        _ => match cache.get_setting(SETTING_BACKEND) {
            Some(Value::String(old)) => Some(vec![old]),
            _ => None,
        },
    }
}

/// Read the `--station` options: ID=STATION, or STATION for the first backend.
pub fn stations(values: &[String], first: &str) -> HashMap<String, String> {
    let mut stations = HashMap::new();
    for value in values {
        match value.split_once('=') {
            Some((id, station)) if BACKEND_IDS.contains(&id) => {
                stations.insert(id.to_owned(), station.to_owned())
            }
            _ => stations.insert(first.to_owned(), value.clone()),
        };
    }
    stations
}

/// Where to keep the waiting tracks, `None` to keep none.
pub fn tracks_dir(options: &Options) -> Option<PathBuf> {
    if options.no_cache || options.no_keep_tracks {
        return None;
    }
    Some(
        options
            .tracks_dir
            .clone()
            .unwrap_or_else(|| crate::paths::tracks_dir_of(&options.cache)),
    )
}

/// The options of the command line, from the given arguments (the first is the name of the program).
pub fn parse_args<I, T>(args: I) -> Result<Options, String>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    Options::try_parse_from(args).map_err(|error| error.to_string())
}

/// Log to a file or to stderr; a second call (a logger is already there) does nothing.
pub fn init_logging(debug: bool, file: Option<&Path>) {
    // (symphonia warns of every m4a it probes: the 4 bytes before "ftyp")
    let quiet = "symphonia_core=error,symphonia_format_isomp4=error,symphonia_bundle_mp3=error,symphonia_bundle_flac=error,symphonia_format_ogg=error,symphonia_format_riff=error,symphonia_metadata=error,symphonia_codec_aac=error";
    let level = format!("{},{quiet}", if debug { "debug" } else { "info" });
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| level.as_str().into());
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    let file = file.and_then(|path| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()
    });
    let _ = match file {
        Some(file) => builder
            .with_writer(Mutex::new(file))
            .with_ansi(false)
            .try_init(),
        None => builder.try_init(),
    };
}

/// Move the files of the earlier names (yamusic-radio) to the app's own and tell what was done. Before `run`: it opens them.
pub fn migrate_files() {
    match crate::paths::migrate_legacy(&crate::paths::home()) {
        Ok(done) => done
            .iter()
            .for_each(|line| tracing::info!("Migration: {line}")),
        Err(error) => {
            tracing::error!("Migration of the files of the earlier version failed: {error}")
        }
    }
}

/// Run the daemon until `shutdown` is done. `ready` gets the address of the server.
pub async fn run(
    options: Options,
    ready: Option<tokio::sync::oneshot::Sender<std::net::SocketAddr>>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    // Two connections to one file: the radio owns one, the settings of the backends use the other.
    let open = || TrackCache::open(&options.cache).map_err(std::io::Error::other);
    let (radio_cache, settings) = if options.no_cache {
        (None, None)
    } else {
        (Some(open()?), Some(Arc::new(Mutex::new(open()?))))
    };
    // What the last run was using, unless the command line says otherwise.
    if let (Some(cache), Some(dir)) = (&settings, &options.local_dir) {
        let _ = cache
            .lock()
            .expect("not poisoned")
            .put_setting(SETTING_LOCAL_DIRECTORY, &json!(dir.to_string_lossy()));
    }
    let specs = backend_specs(&options, settings.as_ref(), clock.clone());
    let first = specs[0].id.clone();
    let last = last_backends(settings.as_ref());
    let asked: Vec<String> = options
        .backend
        .iter()
        .cloned()
        .chain(options.local_dir.iter().map(|_| "local".to_owned()))
        .collect();
    let enabled: Vec<String> = asked
        .iter()
        .cloned()
        .chain(last.iter().flatten().cloned())
        .collect();
    let on_change = settings.clone().map(|cache| {
        Box::new(move |ids: &[String]| {
            if let Err(error) = cache
                .lock()
                .expect("not poisoned")
                .put_setting(SETTING_BACKENDS, &json!(ids))
            {
                tracing::error!("Could not keep the backends: {error}");
            }
        }) as Box<dyn Fn(&[String]) + Send + Sync>
    });
    let multi = MultiBackend::new(
        specs,
        &enabled,
        last.is_none(),
        stations(&options.station, &first),
        on_change,
        clock,
    );
    if !asked.is_empty() {
        multi.persist(); // what the command line turned on is kept, too
    }
    let result = serve(
        multi.clone() as Arc<dyn MusicBackend>,
        Some(multi.clone() as Arc<dyn BackendSet>),
        ServeOptions {
            host: options.host.clone(),
            port: options.port,
            station_id: String::new(),
            pool_size: options.tracks,
            effects: effects_level(&options.effects).unwrap_or(0.6),
            mood_arc: options.mood_arc,
            tracks_dir: tracks_dir(&options),
            ffmpeg: options.ffmpeg.clone(),
            ffmpeg_encoders: options.ffmpeg_encoders,
        },
        radio_cache,
        ready,
        shutdown,
    )
    .await;
    multi.aclose().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stations_go_to_their_backend_or_to_the_first() {
        let got = stations(
            &[
                "local=shuffle".into(),
                "user:onyourwave".into(),
                "bogus=x".into(),
            ],
            "yandex",
        );
        assert_eq!(got["local"], "shuffle");
        // "bogus=x" is no backend's: the whole of it is a station of the first
        assert_eq!(got["yandex"], "bogus=x");
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn the_waiting_tracks_are_next_to_the_cache() {
        let mut options = Options::parse_from(["x", "--cache", "/data/radio.db"]);
        assert_eq!(
            tracks_dir(&options),
            Some(PathBuf::from("/data/radio-tracks"))
        );
        options.tracks_dir = Some("/t".into());
        assert_eq!(tracks_dir(&options), Some(PathBuf::from("/t")));
        options.no_keep_tracks = true;
        assert_eq!(tracks_dir(&options), None);
        let options = Options::parse_from(["x", "--no-cache"]);
        assert_eq!(tracks_dir(&options), None);
    }

    #[test]
    fn the_older_setting_keeps_one_backend() {
        let dir = std::env::temp_dir().join(format!("daemon-test-{}", std::process::id()));
        let cache: SharedCache = Arc::new(Mutex::new(TrackCache::open(&dir.join("c.db")).unwrap()));
        assert_eq!(last_backends(Some(&cache)), None);
        cache
            .lock()
            .unwrap()
            .put_setting(SETTING_BACKEND, &json!("local"))
            .unwrap();
        assert_eq!(last_backends(Some(&cache)), Some(vec!["local".to_owned()]));
        cache
            .lock()
            .unwrap()
            .put_setting(SETTING_BACKENDS, &json!(["yandex", 3]))
            .unwrap();
        assert_eq!(last_backends(Some(&cache)), Some(vec!["yandex".to_owned()]));
        let _ = std::fs::remove_dir_all(dir);
    }
}
