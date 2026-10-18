//! A backend for tests: it hands out a fixed list of tracks and keeps a log of what it was asked. Task 3.0.2 of
//! docs/rust-port-radio.md. `tools/radio_harness.py` has the same backend for the Python radio, and the two do the same
//! on the same calls, so a scenario can be run by both and the results compared.
//!
//! How it behaves:
//! * `station_batch` goes round the list of tracks from where it left off and takes the next `batch_size` that are not
//!   excluded. If every track is excluded, it repeats: the next `batch_size` whatever (the station "has nothing else").
//! * `download` copies the file of the track (at once, not on another thread: the downloads of a batch end in the order
//!   they were started in); a track in `fail_downloads` fails.
//! * Every call is written to the log in a fixed form (see [`FakeBackend::calls`]).

use super::{AuthError, AuthPrompt, BackendError, Feedback, MusicBackend, Track};
use async_trait::async_trait;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Mutex,
};

/// A track and the file it is downloaded from.
#[derive(Debug, Clone)]
pub struct FakeTrack {
    pub track: Track,
    pub file: PathBuf,
}

#[derive(Debug, Clone)]
pub struct FakeOptions {
    pub batch_size: usize,
    pub supports_moods: bool,
    pub default_station: String,
    /// The station's (mood_energy, diversity, language) at the start.
    pub settings: (String, String, String),
    /// Ids of tracks that cannot be downloaded.
    pub fail_downloads: HashSet<String>,
    /// Whether the backend can give tracks.
    pub authorized: bool,
}

impl Default for FakeOptions {
    fn default() -> Self {
        Self {
            batch_size: 3,
            supports_moods: false,
            default_station: "fake".to_owned(),
            settings: ("all".to_owned(), "default".to_owned(), "any".to_owned()),
            fail_downloads: HashSet::new(),
            authorized: true,
        }
    }
}

struct State {
    cursor: usize,
    calls: Vec<String>,
    settings: (String, String, String),
    authorized: bool,
    failing: bool,
}

pub struct FakeBackend {
    tracks: Vec<FakeTrack>,
    options: FakeOptions,
    state: Mutex<State>,
    signed_in: tokio::sync::Notify,
}

impl FakeBackend {
    pub fn new(tracks: Vec<FakeTrack>, options: FakeOptions) -> Self {
        let settings = options.settings.clone();
        let authorized = options.authorized;
        Self {
            tracks,
            options,
            state: Mutex::new(State {
                cursor: 0,
                calls: Vec::new(),
                settings,
                authorized,
                failing: false,
            }),
            signed_in: tokio::sync::Notify::new(),
        }
    }

    /// What the backend was asked, in order, one line each:
    /// `batch STATION exclude=ID,ID -> ID,ID`, `download ID ok|fail`, `feedback ID KIND SECONDS` (3 decimals),
    /// `settings STATION`, `set_settings STATION MOOD DIVERSITY LANGUAGE`.
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().expect("not poisoned").calls.clone()
    }

    /// Make the service fail (`station_batch` and `download` answer with a `BackendError`), or work again.
    pub fn set_failing(&self, failing: bool) {
        self.state.lock().expect("not poisoned").failing = failing;
    }

    /// Sign the backend out or in, as if its token went or came.
    pub fn set_authorized(&self, authorized: bool) {
        self.state.lock().expect("not poisoned").authorized = authorized;
        if authorized {
            self.signed_in.notify_waiters();
        }
    }

    fn log(&self, line: String) {
        self.state.lock().expect("not poisoned").calls.push(line);
    }
}

#[async_trait]
impl MusicBackend for FakeBackend {
    fn authorized(&self) -> bool {
        self.state.lock().expect("not poisoned").authorized
    }

    fn auth_prompt(&self) -> AuthPrompt {
        if self.authorized() {
            return AuthPrompt::default();
        }
        AuthPrompt {
            url: Some("https://example.test/login".to_owned()),
            code: Some("CODE-1234".to_owned()),
            fields: vec![super::AuthField {
                name: "token".to_owned(),
                label: "Token".to_owned(),
                secret: true,
            }],
        }
    }

    fn default_station(&self) -> String {
        self.options.default_station.clone()
    }

    async fn wait_authorized(&self) {
        loop {
            let notified = self.signed_in.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.authorized() {
                return;
            }
            notified.await;
        }
    }

    fn start_auth(&self) {
        self.state
            .lock()
            .expect("not poisoned")
            .calls
            .push("start_auth".to_owned());
    }

    async fn authorize(&self, values: &HashMap<String, String>) -> Result<(), AuthError> {
        if self.authorized() {
            return Err(AuthError("the fake backend needs no sign-in".to_owned()));
        }
        if values.get("token").map(String::as_str) == Some("good") {
            self.set_authorized(true);
            return Ok(());
        }
        Err(AuthError("the token is wrong".to_owned()))
    }

    fn supports_moods(&self) -> bool {
        self.options.supports_moods
    }

    async fn station_batch(
        &self,
        station_id: &str,
        exclude_track_ids: &HashSet<String>,
    ) -> Result<Vec<Track>, BackendError> {
        let n = self.tracks.len();
        let mut state = self.state.lock().expect("not poisoned");
        if state.failing {
            state.calls.push(format!("batch {station_id} fails"));
            return Err(BackendError("the service is down".to_owned()));
        }
        let mut picked = Vec::new();
        let mut scanned = 0;
        while picked.len() < self.options.batch_size && scanned < n {
            let track = &self.tracks[state.cursor % n].track;
            state.cursor += 1;
            scanned += 1;
            if !exclude_track_ids.contains(&track.track_id) {
                picked.push(track.clone());
            }
        }
        if picked.is_empty() && n > 0 {
            for _ in 0..self.options.batch_size.min(n) {
                picked.push(self.tracks[state.cursor % n].track.clone());
                state.cursor += 1;
            }
        }
        let mut excluded: Vec<&str> = exclude_track_ids.iter().map(String::as_str).collect();
        excluded.sort_unstable();
        let ids: Vec<&str> = picked.iter().map(|t| t.track_id.as_str()).collect();
        state.calls.push(format!(
            "batch {station_id} exclude={} -> {}",
            excluded.join(","),
            ids.join(",")
        ));
        Ok(picked)
    }

    async fn download(&self, track: &Track, path: &Path) -> Result<(), BackendError> {
        let file = self
            .tracks
            .iter()
            .find(|t| t.track.track_id == track.track_id)
            .map(|t| t.file.clone());
        let failing = self.state.lock().expect("not poisoned").failing;
        let result = match file {
            Some(file) if !failing && !self.options.fail_downloads.contains(&track.track_id) => {
                // A blocking copy, like Python's: the order in which downloads finish is then the order they were started in.
                std::fs::copy(&file, path)
                    .map(|_| ())
                    .map_err(|e| BackendError(e.to_string()))
            }
            _ => Err(BackendError(format!("cannot download {}", track.track_id))),
        };
        self.log(format!(
            "download {} {}",
            track.track_id,
            if result.is_ok() { "ok" } else { "fail" }
        ));
        result
    }

    async fn feedback_track(
        &self,
        track: &Track,
        feedback: Feedback,
        _station_id: &str,
        total_played_seconds: f64,
    ) -> Result<(), BackendError> {
        self.log(format!(
            "feedback {} {} {:.3}",
            track.track_id,
            feedback.as_str(),
            total_played_seconds
        ));
        Ok(())
    }

    async fn station_settings(
        &self,
        station_id: &str,
    ) -> Result<(String, String, String), BackendError> {
        let mut state = self.state.lock().expect("not poisoned");
        state.calls.push(format!("settings {station_id}"));
        Ok(state.settings.clone())
    }

    async fn set_station_settings(
        &self,
        station_id: &str,
        mood_energy: &str,
        diversity: &str,
        language: &str,
    ) -> Result<(), BackendError> {
        let mut state = self.state.lock().expect("not poisoned");
        state.calls.push(format!(
            "set_settings {station_id} {mood_energy} {diversity} {language}"
        ));
        state.settings = (
            mood_energy.to_owned(),
            diversity.to_owned(),
            language.to_owned(),
        );
        Ok(())
    }

    async fn aclose(&self) {}
}
