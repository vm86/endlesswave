//! What the radio needs from a music service, and nothing Yandex-specific. Task 3.0.1 of docs/rust-port-radio.md,
//! ported from `backend.py`.
//!
//! The radio only talks to a [`MusicBackend`]: it asks for batches of tracks from a "station", downloads them to
//! files, and tells the backend what was heard. Yandex Music is one implementation; another service or a local
//! collection only has to provide the same few calls.

pub mod fake;
pub mod local;
pub mod multi;
pub mod yandex;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::Path,
};

/// The backend could not do what it was asked: it or its service is down. Backends return it (and nothing else) for
/// what may pass; the multi-backend turns to the other backends when it sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError(pub String);

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BackendError {}

/// The backend turned the sign-in down (or has no such way to sign in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthError(pub String);

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AuthError {}

/// A value the user types to sign in: a login, a password, a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthField {
    pub name: String,
    pub label: String,
    #[serde(default)]
    pub secret: bool,
    /// The values to choose from, when the field is a choice and not a text; none for a text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<Choice>,
}

/// One of the values of a field that is a choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Choice {
    pub value: String,
    pub label: String,
}

/// What the user is asked to do to sign in, as far as it is known. A backend asks for either or both of these: to open
/// `url` and enter `code` there (a device flow; both stay `None` until the service has given them), and to fill in
/// `fields`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthPrompt {
    pub url: Option<String>,
    pub code: Option<String>,
    #[serde(default)]
    pub fields: Vec<AuthField>,
}

/// What happened to a track, as the backend is told about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feedback {
    Started,
    Finished,
    Skipped,
}

impl Feedback {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Finished => "finished",
            Self::Skipped => "skipped",
        }
    }
}

/// A track of a backend, ready to be downloaded. `track_id` is unique within the backend. `codec` (the file extension
/// that ffmpeg is given) and `bitrate_in_kbps` describe what `download` will write and are part of the key of the
/// analysis cache. `batch_id` is opaque: the backend gets it back with the feedback.
///
/// The fields and their names are those of `dataclasses.asdict(Track)` of the Python radio: the playlist kept in the
/// cache holds tracks in this form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Track {
    pub track_id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub codec: String,
    #[serde(default)]
    pub bitrate_in_kbps: i64,
    #[serde(default)]
    pub genre: String,
    /// The URL of the cover with %% where the size ("400x400") goes, or "".
    #[serde(default)]
    pub cover: String,
    #[serde(default)]
    pub batch_id: String,
    /// The mood the backend knows the track by (one of `choose::ALL_MOODS`), or "" when it doesn't: the radio then
    /// judges by the sound of the track.
    #[serde(default)]
    pub mood: String,
    /// Set by the multi-backend: the backend the track came from, and the id the backend knows it by when that is not
    /// `track_id`.
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub native_id: String,
}

/// A source of tracks for the radio.
#[async_trait]
pub trait MusicBackend: Send + Sync {
    /// Whether the backend can give tracks (it has a token, say).
    fn authorized(&self) -> bool;
    /// What the user has to do to authorize the app, while it isn't.
    fn auth_prompt(&self) -> AuthPrompt;
    /// The station to play when none is asked for.
    fn default_station(&self) -> String;
    /// Return once the backend is authorized (at once if it is).
    async fn wait_authorized(&self);
    /// Begin authorizing, if that goes on by itself (a device flow). Called when the backend is chosen and isn't
    /// authorized. Does nothing if it has started or isn't needed.
    fn start_auth(&self);
    /// Sign in with the values of `auth_prompt().fields`.
    async fn authorize(&self, values: &HashMap<String, String>) -> Result<(), AuthError>;
    /// Whether `set_station_settings` can steer the mood of a station. Without it the radio finds the mood of each
    /// track by itself.
    fn supports_moods(&self) -> bool;
    /// The next tracks of the station, ready to be downloaded. Tracks of `exclude_track_ids` may be left out. May
    /// repeat tracks it gave before when it has nothing else. `BackendError` when the service can't be reached.
    async fn station_batch(
        &self,
        station_id: &str,
        exclude_track_ids: &HashSet<String>,
    ) -> Result<Vec<Track>, BackendError>;
    /// Write the audio of the track to `path`.
    async fn download(&self, track: &Track, path: &Path) -> Result<(), BackendError>;
    /// Tell the backend that the track was started, finished or skipped.
    async fn feedback_track(
        &self,
        track: &Track,
        feedback: Feedback,
        station_id: &str,
        total_played_seconds: f64,
    ) -> Result<(), BackendError>;
    /// The station's (mood_energy, diversity, language).
    async fn station_settings(
        &self,
        station_id: &str,
    ) -> Result<(String, String, String), BackendError>;
    /// Change the station's settings (only if `supports_moods`).
    async fn set_station_settings(
        &self,
        station_id: &str,
        mood_energy: &str,
        diversity: &str,
        language: &str,
    ) -> Result<(), BackendError>;
    /// Stop what the backend runs in the background.
    async fn aclose(&self);
    /// The settings the page can show and change (the folder of `local`), with what they are called; `None` for a backend
    /// that has none. The new values are given to `authorize`, which keeps them.
    fn settings(&self) -> Option<(HashMap<String, String>, Vec<AuthField>)> {
        None
    }
    /// Change the settings that `settings` lists (not the sign-in: that is `authorize`). `values` holds the ones to change.
    async fn update_settings(&self, _values: &HashMap<String, String>) -> Result<(), AuthError> {
        Err(AuthError(
            "this music backend has no settings to change".to_owned(),
        ))
    }
    /// The ids of the backends that are off or can't be reached; the tracks of them that are only linked leave the
    /// queue. Only the multi-backend has any.
    fn unavailable(&self) -> Vec<String> {
        Vec::new()
    }
}
