//! The radio inside the app: `radio-core` runs on a thread of its own, with a runtime of its own, instead of the
//! `yamusic-dj` child process. Stage 6 of docs/rust-port.md. Chosen with ENDLESSWAVE_ENGINE=rust until it replaces the
//! Python daemon.

use std::path::{Path, PathBuf};
use std::time::Duration;

use radio_core::daemon::{init_logging, parse_args, run};

/// The engine that runs: stopped by `stop`, or found ended by `is_finished` (a crash).
pub struct Engine {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl Engine {
    /// Start the radio with the options of the command line (the program name not included) and wait until it listens.
    pub fn start(args: &[String], ffmpeg: Option<PathBuf>, log: Option<&Path>) -> Result<Self, String> {
        let mut argv = vec!["yamusic-radio".to_owned()];
        argv.extend(args.iter().cloned());
        if let Some(ffmpeg) = ffmpeg {
            argv.push("--ffmpeg".into());
            argv.push(ffmpeg.to_string_lossy().into_owned());
        }
        let options = parse_args(argv)?;
        init_logging(options.debug, log);
        // A panic is told in the log (and ends the thread, which the app finds out and starts the radio again).
        std::panic::set_hook(Box::new(|info| eprintln!("the radio panicked: {info}")));
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let (ready, listening) = std::sync::mpsc::channel::<std::net::SocketAddr>();
        let thread = std::thread::Builder::new()
            .name("radio-engine".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .thread_name("radio-worker")
                    .build()
                    .map_err(|error| error.to_string())?;
                runtime.block_on(async move {
                    let (tell, told) = tokio::sync::oneshot::channel();
                    tokio::spawn(async move {
                        if let Ok(address) = told.await {
                            let _ = ready.send(address);
                        }
                    });
                    run(options, Some(tell), async move {
                        let _ = stopped.await;
                    })
                    .await
                    .map_err(|error| error.to_string())
                })
            })
            .map_err(|error| error.to_string())?;
        let mut engine = Self { stop: Some(stop), thread: Some(thread) };
        // Ready, or ended before it (the port is taken, the cache cannot be opened).
        for _ in 0..200 {
            match listening.recv_timeout(Duration::from_millis(50)) {
                Ok(_) => return Ok(engine),
                Err(_) if engine.is_finished() => break,
                Err(_) => {}
            }
        }
        match engine.join() {
            Err(error) => Err(error),
            Ok(()) => Err("the radio did not start in time".into()),
        }
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|thread| thread.is_finished())
    }

    /// Why the thread ended.
    pub fn join(&mut self) -> Result<(), String> {
        match self.thread.take().map(|thread| thread.join()) {
            Some(Ok(result)) => result,
            Some(Err(_)) => Err("the radio panicked".into()),
            None => Ok(()),
        }
    }

    /// Ask it to end (it puts the settings of the station back first) and wait up to `tenths_of_second`.
    pub fn stop(mut self, tenths_of_second: u32) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        for _ in 0..tenths_of_second {
            if self.is_finished() {
                let _ = self.join();
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // Left to end by itself: the process is about to, or the next start finds the port taken.
        eprintln!("the radio engine did not stop in time");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_radio_listens_inside_the_app_and_stops() {
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let args: Vec<String> = ["--host", "127.0.0.1", "--port", &port.to_string(), "--no-cache"].map(String::from).to_vec();
        let engine = Engine::start(&args, None, None).expect("starts");
        let body: String = ureq::get(&format!("http://127.0.0.1:{port}/api/queue")).call().expect("answers").body_mut().read_to_string().unwrap();
        assert!(body.starts_with('{'), "{body}");
        engine.stop(100);
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err(), "the port is free again");
    }

    #[test]
    fn a_panic_ends_the_engine_and_is_told() {
        let thread = std::thread::spawn(|| -> Result<(), String> { panic!("on purpose") });
        let mut engine = Engine { stop: None, thread: Some(thread) };
        while !engine.is_finished() {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(engine.join(), Err("the radio panicked".to_owned()));
    }

    #[test]
    fn a_taken_port_is_told() {
        let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = taken.local_addr().unwrap().port();
        let args: Vec<String> = ["--host", "127.0.0.1", "--port", &port.to_string(), "--no-cache"].map(String::from).to_vec();
        assert!(Engine::start(&args, None, None).is_err());
    }
}
