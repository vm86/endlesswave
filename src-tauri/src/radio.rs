//! The radio the player listens to: `radio-core`, running inside the app (see `engine`).
//!
//! If something already answers on the port it is used as it is; otherwise the radio is started on a thread of its
//! own and stopped with the player.

use std::net::{SocketAddr, TcpStream};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::engine::Engine;
use tauri::{AppHandle, Emitter, Manager};

pub const HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 18000;

#[derive(Default)]
pub struct Radio {
    engine: Mutex<Option<Engine>>,
    /// Set when the app is leaving: a service that ends then is not a crash.
    stopping: AtomicBool,
    watching: AtomicBool,
    /// The service is stopped on purpose (the reset): the watcher does not start it again.
    paused: AtomicBool,
}

/// A service that lived longer than this was working: its end starts the count of crashes anew.
const STABLE: Duration = Duration::from_secs(60);
/// So many crashes in a row and the service is given up.
const MAX_CRASHES: u32 = 5;

/// A setting of the environment: ENDLESSWAVE_<name>.
fn env(name: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(format!("ENDLESSWAVE_{name}"))
}

pub fn port() -> u16 {
    env("PORT")
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

pub fn url() -> String {
    format!("http://{HOST}:{}", port())
}

fn is_up(port: u16) -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
}

/// The file of the log, `~/.cache/endlesswave.log`.
fn log_path() -> Option<PathBuf> {
    std::env::var_os("HOME")?;
    let path = radio_core::paths::log_file();
    fs::create_dir_all(path.parent()?).ok()?;
    Some(path)
}

/// The ffmpeg bundled with the app first, then PATH, then where ffmpeg usually
/// is (Homebrew on Apple Silicon and Intel, MacPorts).
fn path_with_ffmpeg_places(resources: Option<&Path>) -> std::ffi::OsString {
    let mut paths: Vec<PathBuf> = resources
        .map(|dir| vec![dir.join("ffmpeg")])
        .unwrap_or_default();
    paths.extend(
        std::env::var_os("PATH")
            .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
            .unwrap_or_default(),
    );
    for extra in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin"] {
        let extra = PathBuf::from(extra);
        if !paths.contains(&extra) {
            paths.push(extra);
        }
    }
    std::env::join_paths(paths).unwrap_or_default()
}

fn cache_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".cache"))
}

/// The settings (the queue of the last run, the moods, the genres, the sources...), the history of the
/// listening (when a track was played, the blends of the statistics) and the tracks that were loaded to
/// wait for their turn. The analysis of the tracks stays, so does the token.
fn clean_state() -> Result<(), String> {
    let cache = cache_dir().ok_or("no home directory")?;
    let db = cache.join(radio_core::paths::CACHE_NAME);
    if db.exists() {
        let mut conn = rusqlite::Connection::open(&db).map_err(|error| format!("cannot open the database: {error}"))?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(|error| error.to_string())?;
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        tx.execute_batch("DELETE FROM settings; DELETE FROM transitions; UPDATE tracks SET played_at = NULL;")
            .map_err(|error| format!("cannot clear the database: {error}"))?;
        tx.commit().map_err(|error| error.to_string())?;
    }
    if let Ok(entries) = fs::read_dir(radio_core::paths::tracks_dir_of(&db)) {
        for entry in entries.flatten() {
            let path = entry.path();
            let _ = if path.is_dir() { fs::remove_dir_all(&path) } else { fs::remove_file(&path) };
        }
    }
    Ok(())
}

/// Looks at the service we started: if it ends while the app lives, starts it again, with a pause that
/// grows (1, 2, 4... up to 30 s), and gives up after a few crashes in a row. The page is told with
/// the event "radio" ("restarting", "failed"), so it can say so instead of an error.
fn watch(app: &AppHandle) {
    let radio = app.state::<Radio>();
    let mut launched = Instant::now();
    let mut crashes = 0u32;
    loop {
        std::thread::sleep(Duration::from_secs(1));
        if radio.stopping.load(Ordering::SeqCst) {
            return;
        }
        if radio.paused.load(Ordering::SeqCst) {
            continue;
        }
        let mut died = None;
        {
            let mut engine = radio.engine.lock().unwrap();
            if engine.as_ref().is_some_and(Engine::is_finished) {
                let mut ended = engine.take().expect("checked above");
                died = Some(ended.join().err().unwrap_or_else(|| "it ended".into()));
            }
        }
        let Some(status) = died else { continue };
        if launched.elapsed() >= STABLE {
            crashes = 0;
        }
        crashes += 1;
        eprintln!("the radio service ended ({status}), crash {crashes} of {MAX_CRASHES}");
        if crashes >= MAX_CRASHES {
            let _ = app.emit("radio", "failed");
            return;
        }
        let _ = app.emit("radio", "restarting");
        std::thread::sleep(Duration::from_secs(1 << (crashes - 1).min(5)).min(Duration::from_secs(30)));
        if radio.stopping.load(Ordering::SeqCst) {
            return;
        }
        let resources = app.path().resource_dir().ok();
        match radio.launch(resources.as_deref()) {
            Ok(()) => launched = Instant::now(),
            Err(error) => {
                eprintln!("cannot start the radio service again: {error}");
                let _ = app.emit("radio", "failed");
                return;
            }
        }
    }
}

impl Radio {
    /// Starts the service (if none answers yet) and keeps watching it: if it dies, it is started again.
    pub fn start(&self, app: &AppHandle) -> Result<(), String> {
        let resources = app.path().resource_dir().ok();
        let result = self.launch(resources.as_deref());
        if !self.watching.swap(true, Ordering::SeqCst) {
            let app = app.clone();
            std::thread::spawn(move || watch(&app));
        }
        result
    }

    /// Start `radio-core` on a thread of the app, unless something answers on the port already.
    fn launch(&self, resources: Option<&Path>) -> Result<(), String> {
        let port = port();
        let mut guard = self.engine.lock().unwrap();
        if guard.is_some() {
            return Ok(());
        }
        if is_up(port) {
            return Ok(());
        }
        // An app started from the Finder has a short PATH: ffprobe and the rest are looked for by it.
        // Safety: set before the engine starts its threads, from the one that starts it.
        unsafe { std::env::set_var("PATH", path_with_ffmpeg_places(resources)) };
        let mut args: Vec<String> = ["--host", HOST, "--port"].map(String::from).to_vec();
        args.push(port.to_string());
        if let Some(extra) = env("ARGS").and_then(|value| value.into_string().ok()) {
            args.extend(extra.split_whitespace().map(String::from));
        }
        if env("DEBUG").is_some() {
            args.push("--debug".into());
        }
        let bundled = resources.map(|dir| dir.join("ffmpeg").join("ffmpeg")).filter(|path| path.is_file());
        let log = (env("LOG").is_some() || env("DEBUG").is_some()).then(log_path).flatten();
        *guard = Some(Engine::start(&args, bundled, log.as_deref())?);
        Ok(())
    }

    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.stop_child(40);
    }

    /// The reset: the queue, the settings and the history of the radio are cleared, and the radio starts again
    /// with none of them. What stays: the sign-in (the file of the config is not touched) and the cache of the
    /// analysis of the tracks (the table `tracks`, but for the time when a track was played).
    ///
    /// The service has to stop first: it writes its state when it ends. One the app did not start
    /// (it was there before) is not ours to stop.
    pub fn reset(&self, resources: Option<&Path>) -> Result<(), String> {
        if self.engine.lock().unwrap().is_none() && is_up(port()) {
            return Err("The radio service was not started by this app: stop it, then reset".into());
        }
        self.paused.store(true, Ordering::SeqCst);
        self.stop_child(150); // it ends by itself in a moment once nobody listens
        let cleaned = clean_state();
        self.paused.store(false, Ordering::SeqCst);
        let started = self.launch(resources);
        cleaned.and(started)
    }

    fn stop_child(&self, tenths_of_second: u32) {
        if let Some(engine) = self.engine.lock().unwrap().take() {
            engine.stop(tenths_of_second);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_clears_the_state_and_keeps_the_analysis() {
        let home = std::env::temp_dir().join(format!("endlesswave-test-{}", std::process::id()));
        let cache = home.join(".cache");
        fs::create_dir_all(cache.join("endlesswave-tracks")).unwrap();
        fs::write(cache.join("endlesswave-tracks").join("a.aac"), b"x").unwrap();
        // Safety: the only test that touches the environment.
        unsafe { std::env::set_var("HOME", &home) };

        let conn = rusqlite::Connection::open(cache.join("endlesswave.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE tracks (track_id TEXT, analysis TEXT, played_at REAL);
             CREATE TABLE transitions (at REAL);
             CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO tracks VALUES ('a', '{\"bpm\": 120}', 5.0);
             INSERT INTO transitions VALUES (1.0);
             INSERT INTO settings VALUES ('playlist', '[]');",
        )
        .unwrap();
        drop(conn);

        clean_state().unwrap();

        let conn = rusqlite::Connection::open(cache.join("endlesswave.db")).unwrap();
        let count = |sql: &str| conn.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT COUNT(*) FROM settings"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM transitions"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM tracks"), 1, "the analysis stays");
        assert_eq!(count("SELECT COUNT(*) FROM tracks WHERE played_at IS NULL"), 1, "the history goes");
        assert_eq!(count("SELECT COUNT(*) FROM tracks WHERE analysis LIKE '%120%'"), 1);
        assert_eq!(fs::read_dir(cache.join("endlesswave-tracks")).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&home);
    }
}
