//! The yamusic-radio daemon (`yamusic-dj`) the player listens to.
//!
//! If something already answers on the port it is used as it is; otherwise the
//! daemon is started in the foreground as a child and stopped with the player.

use std::net::{SocketAddr, TcpStream};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

pub const HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 18000;

#[derive(Default)]
pub struct Radio {
    child: Mutex<Option<Child>>,
}

pub fn port() -> u16 {
    std::env::var("YAMUSIC_RADIO_PORT")
        .ok()
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

/// `YAMUSIC_DJ_BIN`, then the daemon bundled with the app, then the sibling
/// checkout's venv (development), then PATH.
fn binary(resources: Option<&Path>) -> PathBuf {
    if let Some(path) = std::env::var_os("YAMUSIC_DJ_BIN") {
        return path.into();
    }
    if let Some(bundled) = resources.map(|dir| dir.join("backend").join("yamusic-dj")) {
        if bundled.is_file() {
            return bundled;
        }
    }
    let sibling = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../yamusic-radio/.venv/bin/yamusic-dj");
    if cfg!(debug_assertions) && sibling.exists() {
        return sibling;
    }
    "yamusic-dj".into()
}

/// Where the daemon's log goes: the file it uses itself when it runs alone.
fn log_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home).join(".cache");
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join("yamusic_radio.log"))
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

impl Radio {
    pub fn start(&self, resources: Option<&Path>) -> Result<(), String> {
        let port = port();
        if is_up(port) {
            return Ok(());
        }
        let mut guard = self.child.lock().unwrap();
        if guard.is_some() {
            return Ok(());
        }
        let bin = binary(resources);
        // In the foreground the daemon logs to stderr: keep it in the file.
        let log = log_path()
            .and_then(|path| OpenOptions::new().create(true).append(true).open(path).ok())
            .map(Stdio::from)
            .unwrap_or_else(Stdio::null);
        let mut command = Command::new(&bin);
        // An app started from the Finder has a short PATH: the daemon needs ffmpeg.
        command.env("PATH", path_with_ffmpeg_places(resources));
        command
            .args(["--foreground", "--host", HOST, "--port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        // Anything else the daemon takes, e.g. YAMUSIC_RADIO_ARGS="--no-keep-tracks".
        if let Ok(extra) = std::env::var("YAMUSIC_RADIO_ARGS") {
            command.args(extra.split_whitespace());
        }
        if std::env::var_os("YAMUSIC_RADIO_DEBUG").is_some() {
            command.arg("--debug");
        }
        let child = command
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", bin.display()))?;
        *guard = Some(child);
        Ok(())
    }

    pub fn stop(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            // SIGTERM first: the daemon puts the station's settings back and
            // keeps the loaded tracks, which takes a moment.
            #[cfg(unix)]
            {
                let _ = Command::new("kill").arg(child.id().to_string()).status();
                for _ in 0..80 {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
