//! The radio: a pool of prepared tracks, the choice of the next one, the queue, and what the API shows. Stage 3.1 of
//! docs/rust-port-radio.md, ported from `class Radio` of `dj.py`.
//!
//! The state is behind one `std::sync::Mutex` that is never held across an `.await`; the long work (downloads, decoding,
//! analysis) is done outside it. What the Python radio waits on with `asyncio.Event` is a `tokio::sync::Notify` here.

mod load;
mod pool;
mod report;
mod types;

pub use types::{Candidate, Counter, MAX_LISTED, Played, RadioError, TrackInfo};

use crate::{
    backend::{Feedback, MusicBackend, Track},
    cache::TrackCache,
    clock::Clock,
    mix::{TrackAnalysis, key},
    pipeline::{BlendReport, Deck, DeckError, DeckSource},
};
use async_trait::async_trait;
use indexmap::IndexMap;
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::sync::{Notify, Semaphore};

pub const DEFAULT_POOL_SIZE: usize = 12;
/// Tracks downloaded and analyzed at the same time.
pub const PREPARE_CONCURRENCY: usize = 3;
/// Tracks decoded and analyzed at a time: each takes a few times its size in memory, a decoded six-minute track being
/// 127 MB. Downloads go on meanwhile.
pub const ANALYZE_CONCURRENCY: usize = 1;
/// Give up filling the pool after this many batches without a usable track: the station only repeats itself.
pub const MAX_EMPTY_BATCHES: usize = 5;
/// Batches of the station with nothing of the wanted genres in a row.
pub const MAX_OFF_GENRE_BATCHES: usize = 20;
/// When no track in the pool fits the one playing, load this many more...
pub const EXTRA_TRACKS: usize = 3;
/// ...up to this many, and take the best there is after that.
pub const MAX_EXTRA_TRACKS: usize = 6;
/// While the station gives nothing new, look again this often.
pub const REPLENISH_RETRY_SECONDS: u64 = 120;
/// Played tracks that can be repeated, oldest forgotten first.
pub const MAX_PLAYED_REMEMBERED: usize = 500;
/// The blends kept to look at (see `recent_blends`).
pub const MAX_RECENT_BLENDS: usize = 10;
/// Tracks played before a restart that the radio does not pick again.
pub const CACHED_PLAYED_SKIPPED: i64 = 200;
/// When the language is changed, the tracks loaded for the old one are kept only as far as they are needed for the next
/// blends.
pub const LANGUAGE_KEEP_TRACKS: usize = 2;
/// The languages of the music a station can be set to (Yandex's own names).
pub const LANGUAGES: [&str; 4] = ["any", "russian", "not-russian", "without-words"];
/// What is kept in the cache across restarts.
pub const SETTING_MOODS: &str = "moods";
pub const SETTING_GENRES: &str = "genres";
pub const SETTING_BLOCKED: &str = "blocked_genres";
pub const SETTING_STATION: &str = "station_settings";
pub const SETTING_LANGUAGE: &str = "language";
/// The tracks loaded and waiting for their turn: the files are kept as well.
pub const SETTING_PLAYLIST: &str = "playlist";
/// The order of the queue that was put in place by hand.
pub const SETTING_ORDER: &str = "queue_order";
/// What a download can be called, of the ones that are cleared away.
pub const AUDIO_SUFFIXES: [&str; 12] =
    ["mp3", "flac", "flac-mp4", "aac", "aac-mp4", "he-aac", "he-aac-mp4", "m4a", "mp4", "ogg", "opus", "wav"];
/// The genres of My Wave known so far: offered while the cache has none yet.
pub const KNOWN_GENRES: [&str; 41] = [
    "dance", "pop", "house", "electronics", "rock", "country", "alternative", "hardrock", "numetal", "alternativemetal",
    "classicmetal", "industrial", "metal", "epicmetal", "lounge", "edmgenre", "gothicmetal", "indie", "punk", "relax", "rnb",
    "estrada", "metalcoregenre", "rap", "soundtrack", "trance", "tvseries", "disco", "dnb", "films", "folkmetal", "folkrock",
    "kpop", "newwave", "posthardcore", "ruspop", "rusrock", "soul", "stonerrock", "triphopgenre", "videogame",
];

/// What the radio is told when it is made.
#[derive(Debug, Clone)]
pub struct RadioConfig {
    /// The station asked for; "" for the one the backend starts with.
    pub station_id: String,
    pub pool_size: usize,
    pub mood_arc: bool,
    /// The loaded tracks are kept in the cache (the files stay in the working directory, which must be a place that
    /// lasts) for the next run to start on.
    pub keep_playlist: bool,
    pub ffmpeg: PathBuf,
}

impl Default for RadioConfig {
    fn default() -> Self {
        Self { station_id: String::new(), pool_size: DEFAULT_POOL_SIZE, mood_arc: false, keep_playlist: false, ffmpeg: PathBuf::from("ffmpeg") }
    }
}

/// Everything the radio changes while it runs.
pub(crate) struct State {
    pub pool: Vec<Candidate>,
    /// Artists of the tracks played so far (the last few), and how many there were.
    pub history: VecDeque<HashSet<String>>,
    pub position: i64,
    /// Tracks already played, the one played longest ago first. They are never picked again until the station has nothing
    /// new.
    pub played: IndexMap<String, Played>,
    /// Tracks turned down for their mood, counted by the loading.
    pub off_mood: u64,
    /// The mood setting the station has now.
    pub mood: String,
    /// The language setting the station has now ("" before the settings are put aside) and the one picked here, `None`
    /// for the account's own.
    pub language: String,
    pub picked_language: Option<String>,
    pub saved_settings: Option<(String, String, String)>,
    pub playing_id: String,
    /// What the API shows: the track heard now, the ones before it and the one already decoded to come next.
    pub now: Option<Deck>,
    pub now_since: f64,
    /// The mix is paused: by the API, or because nobody listens (held). The time of the track that plays does not run
    /// meanwhile.
    pub user_paused: bool,
    pub held: bool,
    pub paused_at: Option<f64>,
    /// The next track was asked for; a transition is under way (from its count to the end of the track that is left);
    /// and how long the track left was heard, if it was skipped.
    pub skip: bool,
    pub blending: bool,
    pub heard_seconds: Option<f64>,
    pub next: Option<Deck>,
    /// The order of the tracks put in place by hand (their ids), first to play first; the rest of the queue is the
    /// radio's choice.
    pub order: Vec<String>,
    pub before: VecDeque<TrackInfo>,
    /// Only these station moods are played; empty is any.
    pub allowed_moods: Vec<String>,
    /// Genres picked on the page, and the ones the queue is built from: the first become the second only when the queue
    /// is rebuilt.
    pub wanted_genres: Vec<String>,
    pub allowed_genres: Vec<String>,
    /// Genres that are never played: picked, and in force (the same way).
    pub wanted_blocked: Vec<String>,
    pub allowed_blocked: Vec<String>,
    /// Tracks of other genres that the station offered: not asked again for as long as the genres stay the same.
    pub off_genre: HashSet<String>,
    /// Tracks that were fetched or played; not picked again. What was played before this run comes from the cache.
    pub seen: HashSet<String>,
    /// Counts the rebuilds of the queue: a deck chosen before the last one is stale.
    pub generation: u64,
    /// What the transitions of this run were like.
    pub transitions: Counter,
    pub effects: Counter,
    pub recent: VecDeque<BlendReport>,
    pub transition_keys: Counter,
    pub matched_transitions: i64,
    pub key_pairs: IndexMap<(String, String), i64>,
}

pub struct Radio {
    pub(crate) backend: Arc<dyn MusicBackend>,
    pub(crate) cache: Option<Mutex<TrackCache>>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) workdir: PathBuf,
    pub(crate) config: RadioConfig,
    /// Whether the playlist is kept (it needs a cache).
    pub(crate) keep_playlist: bool,
    pub(crate) state: Mutex<State>,
    pub(crate) slots: Semaphore,
    pub(crate) analyzing: Semaphore,
    pub(crate) load_lock: tokio::sync::Mutex<()>,
    pub(crate) feedback_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Tracks were taken: the refill looks again.
    pub(crate) consumed: Notify,
    resumed: Notify,
}

impl Radio {
    pub fn new(backend: Arc<dyn MusicBackend>, workdir: PathBuf, config: RadioConfig, cache: Option<TrackCache>, clock: Arc<dyn Clock>) -> Arc<Self> {
        tracing::info!("Keys are found by {}", crate::mix::key_detector());
        let seen = cache.as_ref().and_then(|c| c.recently_played(CACHED_PLAYED_SKIPPED).ok()).unwrap_or_default();
        let keep_playlist = config.keep_playlist && cache.is_some();
        Arc::new(Self {
            backend,
            cache: cache.map(Mutex::new),
            clock,
            workdir,
            keep_playlist,
            config,
            state: Mutex::new(State {
                pool: Vec::new(),
                history: VecDeque::new(),
                position: 0,
                played: IndexMap::new(),
                off_mood: 0,
                mood: String::new(),
                language: String::new(),
                picked_language: None,
                saved_settings: None,
                playing_id: String::new(),
                now: None,
                now_since: 0.0,
                user_paused: false,
                held: false,
                paused_at: None,
                skip: false,
                blending: false,
                heard_seconds: None,
                next: None,
                order: Vec::new(),
                before: VecDeque::new(),
                allowed_moods: Vec::new(),
                wanted_genres: Vec::new(),
                allowed_genres: Vec::new(),
                wanted_blocked: Vec::new(),
                allowed_blocked: Vec::new(),
                off_genre: HashSet::new(),
                seen,
                generation: 0,
                transitions: Counter::default(),
                effects: Counter::default(),
                recent: VecDeque::new(),
                transition_keys: Counter::default(),
                matched_transitions: 0,
                key_pairs: IndexMap::new(),
            }),
            slots: Semaphore::new(PREPARE_CONCURRENCY),
            analyzing: Semaphore::new(ANALYZE_CONCURRENCY),
            load_lock: tokio::sync::Mutex::new(()),
            feedback_tasks: Mutex::new(Vec::new()),
            consumed: Notify::new(),
            resumed: Notify::new(),
        })
    }

    pub(crate) fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("not poisoned")
    }

    /// The cache, if there is one.
    pub(crate) fn cache(&self) -> Option<MutexGuard<'_, TrackCache>> {
        self.cache.as_ref().map(|c| c.lock().expect("not poisoned"))
    }

    /// The station asked for, else the one the backend starts with.
    pub fn station_id(&self) -> String {
        if self.config.station_id.is_empty() { self.backend.default_station() } else { self.config.station_id.clone() }
    }

    pub fn authorized(&self) -> bool {
        self.backend.authorized()
    }

    /// The tracks loaded and waiting (a copy).
    pub fn pool(&self) -> Vec<Candidate> {
        self.st().pool.clone()
    }

    pub fn generation(&self) -> u64 {
        self.st().generation
    }

    // ---- The clock of the track, the pause and the skip --------------------------------------------------------

    /// Seconds of the track that plays, not counting the pauses.
    fn heard(&self, state: &State) -> f64 {
        state.paused_at.unwrap_or_else(|| self.clock.monotonic()) - state.now_since
    }

    /// Stop the time of the track while paused, run it again after.
    fn sync_clock(&self, state: &mut State) {
        let paused = state.user_paused || state.held;
        let now = self.clock.monotonic();
        match (paused, state.paused_at) {
            (true, None) => state.paused_at = Some(now),
            (false, Some(at)) => {
                state.now_since += now - at;
                state.paused_at = None;
            }
            _ => {}
        }
    }

    /// Is the mix standing: paused by the API, or nobody listens.
    pub fn paused(&self) -> bool {
        let state = self.st();
        state.user_paused || state.held
    }

    /// Pause the mix for everyone, until `resume`.
    pub fn pause(&self) {
        let mut state = self.st();
        state.user_paused = true;
        self.sync_clock(&mut state);
    }

    pub fn resume(&self) {
        let mut state = self.st();
        state.user_paused = false;
        self.sync_clock(&mut state);
        drop(state);
        self.resumed.notify_waiters();
    }

    /// Where the pause is: `{"paused": .., "pause_reason": "user" | "no_listeners" | null}`.
    pub fn pause_info(&self) -> serde_json::Value {
        let state = self.st();
        let reason = if state.user_paused {
            Some("user")
        } else if state.held {
            Some("no_listeners")
        } else {
            None
        };
        serde_json::json!({"paused": reason.is_some(), "pause_reason": reason})
    }

    /// Ask for the next track: the blend starts on the next good beat. It is not at once: the next track has to be ready,
    /// and the blend starts on a bar line of the track that plays, as smoothly as any.
    pub fn request_skip(&self) -> Result<(), RadioError> {
        let mut state = self.st();
        if state.now.is_none() {
            return Err(RadioError::SkipRefused("nothing is playing yet".to_owned()));
        }
        if state.blending {
            return Err(RadioError::SkipRefused("a transition is already under way".to_owned()));
        }
        state.skip = true;
        Ok(())
    }

    /// Where the next track button is: asked for, under way, or free.
    pub fn skip_info(&self) -> serde_json::Value {
        let state = self.st();
        serde_json::json!({
            "skip_pending": state.skip,
            "blending": state.blending,
            "can_skip": state.now.is_some() && !state.skip && !state.blending,
        })
    }

    // ---- What the cache keeps ---------------------------------------------------------------------------------

    pub(crate) fn remember(&self, key: &str, value: &serde_json::Value) {
        if let Some(cache) = self.cache() {
            if let Err(error) = cache.put_setting(key, value) {
                tracing::warn!("Could not keep the setting {key}: {error}");
            }
        }
    }

    pub(crate) fn forget(&self, key: &str) {
        if let Some(cache) = self.cache() {
            let _ = cache.delete_setting(key);
        }
    }
}

// ---- Decks and their lifecycle (tasks 3.1.10 and 3.1.11) ---------------------------------------------------------

impl Radio {
    pub(crate) fn send_feedback(&self, deck: &Deck, kind: Feedback, seconds: f64) {
        let Some(track) = deck.track.clone() else { return };
        let (backend, station) = (Arc::clone(&self.backend), self.station_id());
        let handle = tokio::spawn(async move {
            if let Err(error) = backend.feedback_track(&track, kind, &station, seconds).await {
                tracing::warn!("Feedback about {} was not taken: {error}", track.track_id);
            }
        });
        let mut tasks = self.feedback_tasks.lock().expect("not poisoned");
        tasks.retain(|task| !task.is_finished());
        tasks.push(handle);
    }

    /// Wait until the feedback that is on its way has been sent.
    pub async fn wait_feedback(&self) {
        let tasks: Vec<_> = std::mem::take(&mut *self.feedback_tasks.lock().expect("not poisoned"));
        for task in tasks {
            let _ = task.await;
        }
    }
}

/// The deck is the one in `state.next`: decks are shared by their samples.
fn same_deck(a: &Deck, b: &Deck) -> bool {
    Arc::ptr_eq(&a.pcm, &b.pcm)
}

/// The radio as the mix sees it.
#[async_trait]
impl DeckSource for Radio {
    fn skip_requested(&self) -> bool {
        self.st().skip
    }

    async fn next_deck(&self, current: Option<&TrackAnalysis>) -> Result<Deck, DeckError> {
        Radio::next_deck(self, current).await
    }

    fn is_stale(&self, deck: &Deck) -> bool {
        deck.generation != self.st().generation
    }

    fn skip_taken(&self) {
        let mut state = self.st();
        state.skip = false;
        state.heard_seconds = Some(self.heard(&state));
    }

    fn transition(&self, kind: crate::mix::plan::Transition, current: &Deck, following: &Deck, effect: crate::mix::effects::Effect, report: Option<BlendReport>) {
        let relation = key::key_relation(current.analysis.key, following.analysis.key);
        let (from_key, to_key) = (key::camelot_code(current.analysis.key), key::camelot_code(following.analysis.key));
        {
            let mut state = self.st();
            if let Some(report) = report {
                state.recent.push_back(report);
                while state.recent.len() > MAX_RECENT_BLENDS {
                    state.recent.pop_front();
                }
            }
            // Natural blends answer a request for the next track as well.
            state.skip = false;
            state.blending = true;
            state.transitions.add(kind.name());
            state.effects.add(effect.name());
            state.transition_keys.add(relation.name());
            *state.key_pairs.entry((from_key.clone(), to_key.clone())).or_insert(0) += 1;
            state.matched_transitions += i64::from(following.beat_matched);
        }
        if let Some(cache) = self.cache() {
            let id = |deck: &Deck| deck.track.as_ref().map_or_else(String::new, |t| t.track_id.clone());
            let _ = cache.add_transition(&crate::cache::NewTransition {
                kind: kind.name(),
                beat_matched: following.beat_matched,
                key_relation: relation.name(),
                from_track: &id(current),
                to_track: &id(following),
                from_key: &from_key,
                to_key: &to_key,
                effect: effect.name(),
            });
        }
    }

    fn started(&self, deck: &Deck) {
        {
            let mut state = self.st();
            if let Some(now) = state.now.take() {
                let info = TrackInfo::of(&now.title, &now.analysis, now.track.as_ref(), &now.mood, true, false);
                state.before.push_back(info);
                while state.before.len() > MAX_LISTED {
                    state.before.pop_front();
                }
            }
            state.now = Some(deck.clone());
            state.now_since = self.clock.monotonic();
            if state.paused_at.is_some() {
                state.paused_at = Some(state.now_since);
            }
        }
        if let Some(source) = &deck.source {
            let _ = std::fs::remove_file(&source.path);
        }
        let mut changed = false;
        {
            let mut state = self.st();
            if let Some(track) = &deck.track {
                state.order.retain(|id| id != &track.track_id);
            }
            if state.next.as_ref().is_some_and(|next| same_deck(next, deck)) {
                state.next = None;
                changed = true;
            }
        }
        if changed {
            self.save_playlist();
        }
        self.send_feedback(deck, Feedback::Started, 0.0);
    }

    fn finished(&self, deck: &Deck) {
        let heard = {
            let mut state = self.st();
            state.blending = false;
            state.heard_seconds.take()
        };
        match heard {
            Some(seconds) => self.send_feedback(deck, Feedback::Skipped, seconds),
            None => {
                let seconds = (deck.analysis.end - deck.analysis.start) as f64 / f64::from(crate::mix::SAMPLE_RATE);
                self.send_feedback(deck, Feedback::Finished, seconds);
            }
        }
    }

    fn user_paused(&self) -> bool {
        self.st().user_paused
    }

    fn hold(&self, held: bool) {
        let mut state = self.st();
        state.held = held;
        self.sync_clock(&mut state);
    }

    async fn wait_resumed(&self) {
        loop {
            let notified = self.resumed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.st().user_paused {
                return;
            }
            notified.await;
        }
    }
}

/// Track ids of the playing track and the next (for the tests).
impl Radio {
    pub fn now_track(&self) -> Option<Track> {
        self.st().now.as_ref().and_then(|d| d.track.clone())
    }
}
