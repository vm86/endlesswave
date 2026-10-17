//! Playing from several music backends at once. Task 5.1 of docs/rust-port.md, ported from `multi.py`.
//!
//! [`MultiBackend`] is a [`MusicBackend`] made of the backends that were turned on. The radio asks it for batches of tracks
//! and gets them from each backend in turn, so the pool holds tracks of all of them and the usual choice of the next track
//! picks the best fit among them. A backend that fails is left alone for a while (longer each time) and the others carry on.

use super::{AuthError, AuthPrompt, BackendError, Feedback, MusicBackend, Track};
use crate::{api::BackendSet, clock::Clock};
use async_trait::async_trait;
use indexmap::IndexMap;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;

/// A backend that failed this many times in a row is left alone...
pub const FAILURES_TO_REST: u32 = 2;
/// ...for this long, doubling with each failure after that, up to the limit.
pub const REST_SECONDS: f64 = 30.0;
pub const MAX_REST_SECONDS: f64 = 600.0;

/// A music backend that can be turned on.
pub struct BackendSpec {
    pub id: String,
    pub name: String,
    pub create: Box<dyn Fn() -> Arc<dyn MusicBackend> + Send + Sync>,
    /// Whether it has what it needs to play at once (a saved token, say): such backends are turned on without being asked.
    pub configured: Box<dyn Fn() -> bool + Send + Sync>,
    /// The ids of its tracks are used as they are, not with the backend's id in front (for the backend that was there
    /// first: what the cache remembers of its tracks stays valid).
    pub plain_ids: bool,
}

impl BackendSpec {
    pub fn new(id: &str, name: &str, create: impl Fn() -> Arc<dyn MusicBackend> + Send + Sync + 'static) -> Self {
        Self { id: id.to_owned(), name: name.to_owned(), create: Box::new(create), configured: Box::new(|| false), plain_ids: false }
    }

    pub fn configured(mut self, configured: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.configured = Box::new(configured);
        self
    }

    pub fn plain_ids(mut self) -> Self {
        self.plain_ids = true;
        self
    }
}

struct Running {
    id: String,
    name: String,
    plain_ids: bool,
    backend: Arc<dyn MusicBackend>,
    station: String,
    failures: u32,
    rest_until: f64,
}

#[derive(Default)]
struct State {
    running: IndexMap<String, Running>,
    last: String,
}

pub struct MultiBackend {
    specs: IndexMap<String, BackendSpec>,
    stations: HashMap<String, String>,
    on_change: Option<Box<dyn Fn(&[String]) + Send + Sync>>,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    /// Set when a backend is turned on or off.
    changed_event: Notify,
}

/// What a call needs of the backend that owns it, taken out of the state so that nothing is held across an `.await`.
#[derive(Clone)]
struct Owner {
    id: String,
    plain_ids: bool,
    backend: Arc<dyn MusicBackend>,
    station: String,
}

impl MultiBackend {
    /// `enabled`: the ones that were on at the last run. With `auto`, a backend that has what it needs (a token, a folder) is
    /// on without being asked. Not when the ones that were on are known: then a backend that was turned off stays off.
    pub fn new(
        specs: Vec<BackendSpec>,
        enabled: &[String],
        auto: bool,
        stations: HashMap<String, String>,
        on_change: Option<Box<dyn Fn(&[String]) + Send + Sync>>,
        clock: Arc<dyn Clock>,
    ) -> Arc<Self> {
        let me = Arc::new(Self {
            specs: specs.into_iter().map(|spec| (spec.id.clone(), spec)).collect(),
            stations,
            on_change,
            clock,
            state: Mutex::new(State::default()),
            changed_event: Notify::new(),
        });
        let start: Vec<String> = me.specs.values().filter(|spec| enabled.contains(&spec.id) || (auto && (spec.configured)())).map(|spec| spec.id.clone()).collect();
        for id in start {
            me.start(&id);
        }
        me
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("not poisoned")
    }

    fn start(&self, id: &str) {
        let spec = &self.specs[id];
        let backend = (spec.create)();
        let station = self.stations.get(id).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| backend.default_station());
        if !backend.authorized() {
            backend.start_auth();
        }
        self.st().running.insert(
            id.to_owned(),
            Running { id: id.to_owned(), name: spec.name.clone(), plain_ids: spec.plain_ids, backend, station, failures: 0, rest_until: 0.0 },
        );
        self.changed_event.notify_waiters();
        tracing::info!("Music backend on: {}", spec.name);
    }

    fn changed(&self) {
        if let Some(on_change) = &self.on_change {
            let ids: Vec<String> = self.st().running.keys().cloned().collect();
            on_change(&ids);
        }
    }

    /// Keep which backends are on, as they are now.
    pub fn persist(&self) {
        self.changed();
    }

    fn running(&self) -> Vec<Owner> {
        self.st().running.values().map(owner).collect()
    }

    /// The backends to ask, the next one in turn first.
    fn usable(&self) -> Vec<Owner> {
        let now = self.clock.monotonic();
        let state = self.st();
        let signed_in: Vec<&Running> = state.running.values().filter(|r| r.backend.authorized()).collect();
        let rested: Vec<&Running> = signed_in.iter().copied().filter(|r| now >= r.rest_until).collect();
        // All are resting: try the one that rests the shortest, rather than sit idle.
        let mut chosen: Vec<&Running> = if rested.is_empty() {
            let mut resting = signed_in;
            resting.sort_by(|a, b| a.rest_until.total_cmp(&b.rest_until));
            resting.into_iter().take(1).collect()
        } else {
            rested
        };
        if let Some(at) = chosen.iter().position(|r| r.id == state.last) {
            let n = chosen.len();
            chosen.rotate_left((at + 1) % n);
        }
        chosen.into_iter().map(owner).collect()
    }

    fn failed(&self, id: &str, error: &BackendError) {
        let now = self.clock.monotonic();
        let mut state = self.st();
        let Some(running) = state.running.get_mut(id) else { return };
        running.failures += 1;
        tracing::warn!("{} failed ({} in a row): {error}", running.name, running.failures);
        if running.failures >= FAILURES_TO_REST {
            let extra = running.failures - FAILURES_TO_REST;
            let rest = (REST_SECONDS * 2f64.powi(extra as i32)).min(MAX_REST_SECONDS);
            running.rest_until = now + rest;
            tracing::warn!("{} rests for {rest:.0} s", running.name);
        }
    }

    fn worked(&self, id: &str) {
        if let Some(running) = self.st().running.get_mut(id) {
            if running.failures != 0 {
                tracing::info!("{} works again", running.name);
            }
            running.failures = 0;
            running.rest_until = 0.0;
        }
    }

    fn owner_of(&self, track: &Track) -> Result<Owner, BackendError> {
        self.st()
            .running
            .get(&track.source)
            .map(owner)
            .ok_or_else(|| BackendError(format!("the backend of {} is off", track.track_id)))
    }

    /// Give the track back the way its backend made it.
    fn native(track: &Track) -> Track {
        let mut native = track.clone();
        if !track.native_id.is_empty() {
            native.track_id = track.native_id.clone();
        }
        native.source.clear();
        native.native_id.clear();
        native
    }

    /// Give a backend the ids it knows: its own, without the stamp.
    fn known_to(owner: &Owner, track_ids: &HashSet<String>) -> HashSet<String> {
        if owner.plain_ids {
            return track_ids.clone();
        }
        let stamp = format!("{}:", owner.id);
        track_ids.iter().filter_map(|i| i.strip_prefix(&stamp)).map(str::to_owned).collect()
    }

    fn stamp(owner: &Owner, track: Track) -> Track {
        let mut stamped = track.clone();
        if !owner.plain_ids {
            stamped.track_id = format!("{}:{}", owner.id, track.track_id);
        }
        stamped.native_id = track.track_id;
        stamped.source = owner.id.clone();
        stamped
    }

    /// The backends that are on, signed in, and steer a station by the mood.
    fn steering(&self) -> Vec<Owner> {
        self.st().running.values().filter(|r| r.backend.authorized() && r.backend.supports_moods()).map(owner).collect()
    }
}

fn owner(running: &Running) -> Owner {
    Owner { id: running.id.clone(), plain_ids: running.plain_ids, backend: Arc::clone(&running.backend), station: running.station.clone() }
}

#[async_trait]
impl MusicBackend for MultiBackend {
    fn authorized(&self) -> bool {
        self.st().running.values().any(|r| r.backend.authorized())
    }

    fn auth_prompt(&self) -> AuthPrompt {
        self.st().running.values().find(|r| !r.backend.authorized()).map_or_else(AuthPrompt::default, |r| r.backend.auth_prompt())
    }

    fn default_station(&self) -> String {
        self.st().running.values().map(|r| format!("{}: {}", r.id, r.station)).collect::<Vec<_>>().join(", ")
    }

    fn supports_moods(&self) -> bool {
        !self.steering().is_empty()
    }

    fn start_auth(&self) {
        for running in self.running() {
            if !running.backend.authorized() {
                running.backend.start_auth();
            }
        }
    }

    async fn wait_authorized(&self) {
        while !self.authorized() {
            let changed = self.changed_event.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.authorized() {
                return;
            }
            let mut waits: Vec<std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>> = Vec::new();
            for running in self.running() {
                let backend = Arc::clone(&running.backend);
                waits.push(Box::pin(async move { backend.wait_authorized().await }));
            }
            waits.push(Box::pin(changed));
            futures_util::future::select_all(waits).await;
        }
    }

    async fn authorize(&self, values: &HashMap<String, String>) -> Result<(), AuthError> {
        for running in self.running() {
            if !running.backend.authorized() {
                return running.backend.authorize(values).await;
            }
        }
        Err(AuthError("nothing to sign in to".to_owned()))
    }

    async fn station_batch(&self, _station_id: &str, exclude_track_ids: &HashSet<String>) -> Result<Vec<Track>, BackendError> {
        let mut last_error: Option<BackendError> = None;
        for running in self.usable() {
            match running.backend.station_batch(&running.station, &Self::known_to(&running, exclude_track_ids)).await {
                Ok(batch) => {
                    self.worked(&running.id);
                    self.st().last = running.id.clone();
                    return Ok(batch.into_iter().map(|track| Self::stamp(&running, track)).collect());
                }
                Err(error) => {
                    self.failed(&running.id, &error);
                    last_error = Some(error);
                }
            }
        }
        let _ = last_error; // Python chains it as the cause; the message stays the same
        Err(BackendError("no music backend can give tracks".to_owned()))
    }

    async fn download(&self, track: &Track, path: &Path) -> Result<(), BackendError> {
        let running = self.owner_of(track)?;
        match running.backend.download(&Self::native(track), path).await {
            Ok(()) => {
                self.worked(&running.id);
                Ok(())
            }
            Err(error) => {
                self.failed(&running.id, &error);
                Err(error)
            }
        }
    }

    async fn feedback_track(&self, track: &Track, feedback: Feedback, _station_id: &str, total_played_seconds: f64) -> Result<(), BackendError> {
        let running = self.owner_of(track)?;
        running.backend.feedback_track(&Self::native(track), feedback, &running.station, total_played_seconds).await
    }

    async fn station_settings(&self, _station_id: &str) -> Result<(String, String, String), BackendError> {
        match self.steering().first() {
            None => Ok(("all".to_owned(), "default".to_owned(), "any".to_owned())),
            Some(first) => first.backend.station_settings(&first.station).await,
        }
    }

    async fn set_station_settings(&self, _station_id: &str, mood_energy: &str, diversity: &str, language: &str) -> Result<(), BackendError> {
        let steering = self.steering();
        let mut errors: Vec<BackendError> = Vec::new();
        for running in &steering {
            if let Err(error) = running.backend.set_station_settings(&running.station, mood_energy, diversity, language).await {
                tracing::warn!("{}: no new station settings", running.id);
                errors.push(error);
            }
        }
        if !steering.is_empty() && errors.len() == steering.len() {
            return Err(errors.remove(0));
        }
        Ok(())
    }

    async fn aclose(&self) {
        for running in self.running() {
            running.backend.aclose().await;
        }
    }

    /// The backends that can't give tracks now: they are off, not signed in (a folder that is not there, say), or resting
    /// after failures.
    fn unavailable(&self) -> Vec<String> {
        let now = self.clock.monotonic();
        let state = self.st();
        let mut gone: Vec<String> = self.specs.keys().filter(|i| !state.running.contains_key(*i)).cloned().collect();
        gone.extend(state.running.values().filter(|r| !r.backend.authorized() || now < r.rest_until).map(|r| r.id.clone()));
        gone
    }
}

#[async_trait]
impl BackendSet for MultiBackend {
    async fn enable(&self, id: &str) -> Result<(), String> {
        if !self.specs.contains_key(id) {
            return Err(format!("unknown music backend {}", crate::api::py_repr(id)));
        }
        if self.st().running.contains_key(id) {
            return Ok(());
        }
        self.start(id);
        self.changed();
        Ok(())
    }

    async fn disable(&self, id: &str) -> Result<(), String> {
        if !self.specs.contains_key(id) {
            return Err(format!("unknown music backend {}", crate::api::py_repr(id)));
        }
        let Some(running) = self.st().running.shift_remove(id) else { return Ok(()) };
        running.backend.aclose().await;
        self.changed_event.notify_waiters();
        self.changed();
        tracing::info!("Music backend off: {}", running.name);
        Ok(())
    }

    /// Describe every backend, for the page that turns them on and off.
    fn status(&self) -> Value {
        let now = self.clock.monotonic();
        let state = self.st();
        self.specs
            .values()
            .map(|spec| {
                let mut info = json!({
                    "id": spec.id, "name": spec.name, "enabled": false, "authorized": false, "available": false,
                    "url": null, "code": null, "fields": [], "settings": {}, "setting_fields": [],
                });
                if let Some(running) = state.running.get(&spec.id) {
                    let authorized = running.backend.authorized();
                    let prompt = running.backend.auth_prompt();
                    if let Some((settings, fields)) = running.backend.settings() {
                        info["settings"] = json!(settings);
                        info["setting_fields"] = json!(fields);
                    }
                    info["enabled"] = true.into();
                    info["authorized"] = authorized.into();
                    info["available"] = (authorized && now >= running.rest_until).into();
                    if !authorized {
                        info["url"] = json!(prompt.url);
                        info["code"] = json!(prompt.code);
                        info["fields"] = json!(prompt.fields);
                    }
                }
                info
            })
            .collect()
    }

    /// Sign in to one of the backends that are on.
    async fn authorize_backend(&self, id: &str, values: &HashMap<String, String>) -> Result<(), AuthError> {
        let running = self.st().running.get(id).map(owner);
        let Some(running) = running else { return Err(AuthError(format!("music backend {} is not on", crate::api::py_repr(id)))) };
        running.backend.authorize(values).await
    }
}
