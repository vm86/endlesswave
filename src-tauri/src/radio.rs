//! The yamusic-radio daemon (`yamusic-dj`) the player listens to.
//!
//! If something already answers on the port it is used as it is; otherwise the
//! daemon is started in the foreground as a child and stopped with the player.

use std::net::{SocketAddr, TcpStream};
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};

pub const HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 18000;

#[derive(Default)]
pub struct Radio {
    child: Mutex<Option<Child>>,
    /// Set when the app is leaving: a service that ends then is not a crash.
    stopping: AtomicBool,
    watching: AtomicBool,
}

/// A service that lived longer than this was working: its end starts the count of crashes anew.
const STABLE: Duration = Duration::from_secs(60);
/// So many crashes in a row and the service is given up.
const MAX_CRASHES: u32 = 5;

/// A setting of the environment: ENDLESSWAVE_<name>. The names of the app before it was renamed
/// (YAMUSIC_RADIO_<name>, and YAMUSIC_DJ_BIN) still work.
fn env(name: &str) -> Option<std::ffi::OsString> {
    let old = if name == "DJ_BIN" { "YAMUSIC_DJ_BIN".to_string() } else { format!("YAMUSIC_RADIO_{name}") };
    std::env::var_os(format!("ENDLESSWAVE_{name}")).or_else(|| std::env::var_os(old))
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

/// `ENDLESSWAVE_DJ_BIN`, then the daemon bundled with the app, then the sibling
/// checkout's venv (development), then PATH.
fn binary(resources: Option<&Path>) -> PathBuf {
    if let Some(path) = env("DJ_BIN") {
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

/// The file of the log: the one the daemon uses itself when it runs alone.
fn log_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = PathBuf::from(home).join(".cache");
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join("yamusic_radio.log"))
}

/// In the foreground the daemon logs to stderr. It is not kept: a file that is written all the
/// time grows and is hardly ever read. With ENDLESSWAVE_LOG=1 (or ENDLESSWAVE_DEBUG) it goes to
/// `~/.cache/yamusic_radio.log`; a development build shows it in the terminal.
fn log_target() -> Stdio {
    let wanted = env("LOG").is_some() || env("DEBUG").is_some();
    if wanted {
        if let Some(file) = log_path().and_then(|path| OpenOptions::new().create(true).append(true).open(path).ok()) {
            return Stdio::from(file);
        }
    }
    if cfg!(debug_assertions) { Stdio::inherit() } else { Stdio::null() }
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

/// The pid of the service we started is kept in a file. The app can be killed without a chance to
/// stop it (`kill -9`, a crash; macOS has nothing like Linux's PR_SET_PDEATHSIG), and the next start
/// finds the service that is left by this file.
fn pid_path() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HOME")?).join(".cache");
    fs::create_dir_all(&dir).ok()?;
    // The port is in the name: apps on other ports (ENDLESSWAVE_PORT) keep their own.
    Some(dir.join(format!("endlesswave-{}.pid", port())))
}

fn write_pid(pid: u32) {
    if let Some(path) = pid_path() {
        let _ = fs::write(path, pid.to_string());
    }
}

fn forget_pid() {
    if let Some(path) = pid_path() {
        let _ = fs::remove_file(path);
    }
}

/// Stops the service the file points to if it is still alive: SIGTERM, so it puts the settings of the
/// station back, and SIGKILL if it does not leave in time. The pid may have been given to another
/// process meanwhile: only a process that is a `yamusic-dj` is touched.
#[cfg(unix)]
fn reap_orphan() {
    let Some(path) = pid_path() else { return };
    let Some(pid) = fs::read_to_string(&path).ok().and_then(|text| text.trim().parse::<i32>().ok()) else {
        return;
    };
    let _ = fs::remove_file(&path);
    let alive = |pid: i32| unsafe { libc::kill(pid, 0) == 0 };
    if pid <= 1 || !alive(pid) {
        return;
    }
    let ours = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains("yamusic-dj"))
        .unwrap_or(false);
    if !ours {
        return;
    }
    eprintln!("stopping the radio service {pid} left by an earlier run");
    unsafe { libc::kill(pid, libc::SIGTERM) };
    for _ in 0..80 {
        if !alive(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    unsafe { libc::kill(pid, libc::SIGKILL) };
}

#[cfg(not(unix))]
fn reap_orphan() {}

/// Linux: the system's bundle of the root certificates. The Python inside the package was built
/// elsewhere, and its OpenSSL looks for them in the places of that machine: HTTPS then fails with
/// "unable to get local issuer certificate". Where the bundle is depends on the distribution.
#[cfg(target_os = "linux")]
fn ca_bundle() -> Option<&'static str> {
    [
        "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem", // Fedora, RHEL (the file itself)
        "/etc/pki/tls/certs/ca-bundle.crt",     // Fedora, RHEL (a link to it, may be missing)
        "/etc/ssl/certs/ca-certificates.crt",   // Debian, Ubuntu, Arch
        "/etc/ssl/ca-bundle.pem",               // openSUSE
        "/etc/pki/tls/cacert.pem",              // old RHEL
    ]
    .into_iter()
    .find(|path| Path::new(path).is_file())
}

/// Linux: the kernel sends the service SIGTERM when the app dies, however it dies (a crash, `kill -9`,
/// the session closing). SIGTERM, not SIGKILL: the service then puts the settings of the station back.
///
/// The signal comes when the *thread* that started the child ends, not the process. A command of
/// Tauri runs on a thread that may be gone a minute later, so the child is started from a thread
/// of its own that lives as long as the app does.
#[cfg(target_os = "linux")]
fn spawn(mut command: Command) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt;

    let parent = unsafe { libc::getpid() };
    // Safety: only async-signal-safe calls (prctl, getppid, _exit) between fork and exec.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The app may have died before the call above: the signal would never come.
            if libc::getppid() != parent {
                libc::_exit(1);
            }
            Ok(())
        });
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::Builder::new().name("radio-parent".into()).spawn(move || {
        let _ = sender.send(command.spawn());
        loop {
            std::thread::park();
        }
    })?;
    receiver.recv().map_err(|error| std::io::Error::other(error.to_string()))?
}

#[cfg(not(target_os = "linux"))]
fn spawn(mut command: Command) -> std::io::Result<Child> {
    command.spawn()
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
        let died = match radio.child.lock().unwrap().as_mut() {
            Some(child) => match child.try_wait() {
                Ok(Some(status)) => Some(status.to_string()),
                _ => None,
            },
            None => None, // someone else's service, or not started
        };
        let Some(status) = died else { continue };
        radio.child.lock().unwrap().take();
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

    fn launch(&self, resources: Option<&Path>) -> Result<(), String> {
        let port = port();
        let mut guard = self.child.lock().unwrap();
        if guard.is_some() {
            return Ok(());
        }
        // A service of an earlier run that outlived its app (the app was killed, or crashed).
        reap_orphan();
        if is_up(port) {
            return Ok(());
        }
        let bin = binary(resources);
        let log = log_target();
        let mut command = Command::new(&bin);
        // An app started from the Finder has a short PATH: the daemon needs ffmpeg.
        command.env("PATH", path_with_ffmpeg_places(resources));
        // The user's own SSL_CERT_FILE wins.
        #[cfg(target_os = "linux")]
        if std::env::var_os("SSL_CERT_FILE").is_none() {
            if let Some(bundle) = ca_bundle() {
                command.env("SSL_CERT_FILE", bundle);
            }
        }
        command
            .args(["--foreground", "--host", HOST, "--port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(log);
        // Anything else the daemon takes, e.g. ENDLESSWAVE_ARGS="--no-keep-tracks".
        if let Some(extra) = env("ARGS").and_then(|value| value.into_string().ok()) {
            command.args(extra.split_whitespace());
        }
        if env("DEBUG").is_some() {
            command.arg("--debug");
        }
        let child = spawn(command)
            .map_err(|error| format!("cannot start {}: {error}", bin.display()))?;
        write_pid(child.id());
        *guard = Some(child);
        Ok(())
    }

    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(mut child) = self.child.lock().unwrap().take() {
            forget_pid();
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
