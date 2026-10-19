//! Changing the queue by hand, and picking the genres. Tasks 3.1.5 (the end of it) and 3.1.6 of docs/rust-port-radio.md,
//! ported from `refresh_queue`, `_locked`, `_set_order`, `queue_order`, `queue_move`, `queue_remove`, `queue_play`,
//! `set_wanted_genres`, `set_blocked_genres`, `restore_playlist`, `_candidate_back`.

use super::{
    AUDIO_SUFFIXES, Candidate, Radio, RadioError, SETTING_BLOCKED, SETTING_GENRES, SETTING_ORDER,
    SETTING_PLAYLIST,
};
use crate::backend::Track;
use serde_json::{Value, json};
use std::{collections::HashSet, path::Path};

const FIXED: &str = "a transition is under way: the next track is fixed";

fn unique(genres: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    genres
        .into_iter()
        .filter(|g| seen.insert(g.clone()))
        .collect()
}

impl Radio {
    /// Choose the next tracks anew among the ones already loaded. The track that was already chosen and decoded goes back
    /// to the pool, tracks of moods that are not wanted are dropped, the order put in place by hand is forgotten, and the
    /// next track is picked again, so the queue follows the moods selected now. Nothing is downloaded for it.
    pub fn refresh_queue(&self) {
        let deck = {
            let mut state = self.st();
            state.generation += 1;
            if (&state.allowed_genres, &state.allowed_blocked)
                != (&state.wanted_genres, &state.wanted_blocked)
            {
                state.allowed_genres = state.wanted_genres.clone();
                state.allowed_blocked = state.wanted_blocked.clone();
                state.off_genre.clear();
                tracing::info!(
                    "Genres played: {}; never played: {}",
                    if state.allowed_genres.is_empty() {
                        "any".to_owned()
                    } else {
                        state.allowed_genres.join(", ")
                    },
                    if state.allowed_blocked.is_empty() {
                        "none".to_owned()
                    } else {
                        state.allowed_blocked.join(", ")
                    }
                );
            }
            // What was put in place by hand is the radio's to choose again.
            state.order.clear();
            // The track being blended in stays: it is heard already.
            if state.blending {
                None
            } else {
                state.next.take()
            }
        };
        if let Some(deck) = deck {
            self.unpick(&deck);
        }
        self.drop_unwanted();
        self.save_playlist();
        tracing::info!("Queue rebuilt from {} loaded tracks", self.st().pool.len());
        // Wake up the refill: the pool may be short of tracks now.
        self.consumed.notify_one();
    }

    /// Clear the whole queue and load it anew: the track that plays stays (and the one being blended in), every other track
    /// is dropped, with its file, and the refill fetches tracks from the backends again. The dropped tracks are not marked
    /// as played, so they may come back.
    pub fn clear_queue(&self) {
        let deck = {
            let mut state = self.st();
            state.generation += 1;
            state.order.clear();
            if state.blending {
                None
            } else {
                state.next.take()
            }
        };
        if let Some(deck) = deck {
            // Back to the pool, and out of it with the rest.
            self.unpick(&deck);
        }
        let removed = {
            let mut state = self.st();
            let removed = std::mem::take(&mut state.pool);
            for candidate in &removed {
                if let Some(track) = &candidate.track {
                    state.seen.remove(&track.track_id);
                }
            }
            removed
        };
        for candidate in &removed {
            let _ = std::fs::remove_file(&candidate.path);
        }
        self.save_playlist();
        tracing::info!(
            "Queue cleared: {} tracks dropped, loading anew",
            removed.len()
        );
        self.consumed.notify_one();
    }

    /// Whether the track is the one being blended in just now.
    fn locked(&self, track_id: &str) -> bool {
        let state = self.st();
        state.blending
            && state
                .next
                .as_ref()
                .and_then(|d| d.track.as_ref())
                .is_some_and(|t| t.track_id == track_id)
    }

    /// Put the tracks in this order; what comes first is played first.
    fn set_order(&self, ids: Vec<String>) -> Result<(), RadioError> {
        let coming = self.queue_ids();
        if let Some(unknown) = ids.iter().find(|id| !coming.contains(id)) {
            return Err(RadioError::UnknownQueueTrack(unknown.clone()));
        }
        let ids = unique(ids);
        let (blending_in, decoded) = {
            let state = self.st();
            (
                state.blending && state.next.is_some(),
                state
                    .next
                    .as_ref()
                    .and_then(|d| d.track.as_ref())
                    .map(|t| t.track_id.clone()),
            )
        };
        // The track that is being blended in can't be put behind another.
        if blending_in && !ids.is_empty() && !coming.is_empty() && ids[0] != coming[0] {
            return Err(RadioError::QueueChangeRefused(FIXED.to_owned()));
        }
        self.st().order = ids.clone();
        if !blending_in && !ids.is_empty() && decoded.as_ref().is_some_and(|d| *d != ids[0]) {
            self.restart_next();
        }
        self.save_playlist();
        Ok(())
    }

    /// Put the tracks of the queue in the order of `ids`. The tracks of the queue that are left out follow them, as the
    /// radio chooses.
    pub fn queue_order(&self, ids: Vec<String>) -> Result<(), RadioError> {
        self.set_order(ids)
    }

    /// Move a track of the queue to a place in it (0 is the next).
    pub fn queue_move(&self, track_id: &str, position: i64) -> Result<(), RadioError> {
        let mut ids = self.queue_ids();
        let Some(at) = ids.iter().position(|id| id == track_id) else {
            return Err(RadioError::UnknownQueueTrack(track_id.to_owned()));
        };
        ids.remove(at);
        ids.insert(
            position.clamp(0, ids.len() as i64) as usize,
            track_id.to_owned(),
        );
        self.set_order(ids)
    }

    /// Take a track out of the queue: it is not played, nor loaded again.
    pub fn queue_remove(&self, track_id: &str) -> Result<(), RadioError> {
        if !self.queue_ids().iter().any(|id| id == track_id) {
            return Err(RadioError::UnknownQueueTrack(track_id.to_owned()));
        }
        if self.locked(track_id) {
            return Err(RadioError::QueueChangeRefused(FIXED.to_owned()));
        }
        if self
            .st()
            .next
            .as_ref()
            .and_then(|d| d.track.as_ref())
            .is_some_and(|t| t.track_id == track_id)
        {
            self.restart_next();
        }
        let removed = {
            let mut state = self.st();
            let at = state
                .pool
                .iter()
                .position(|c| c.track.as_ref().is_some_and(|t| t.track_id == track_id));
            at.map(|at| state.pool.remove(at))
        };
        let Some(removed) = removed else {
            return Err(RadioError::UnknownQueueTrack(track_id.to_owned()));
        };
        let _ = std::fs::remove_file(&removed.path);
        if let (Some(cache), Some(track)) = (self.cache(), &removed.track) {
            // Not offered again after a restart either.
            let _ = cache.mark_played(&track.track_id, &track.codec);
        }
        self.st().order.retain(|id| id != track_id);
        self.save_playlist();
        self.consumed.notify_one();
        tracing::info!("Out of the queue: {}", removed.title);
        Ok(())
    }

    /// Take the tracks of one backend out of the queue, the ones that are waiting and not the one that is fixed to come next
    /// (the backend was told to give other tracks). They are not marked as played. Returns how many went.
    pub fn drop_queued_of(&self, source: &str) -> usize {
        let removed: Vec<_> = {
            let mut state = self.st();
            let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut state.pool)
                .into_iter()
                .partition(|c| c.track.as_ref().is_some_and(|t| t.source == source));
            state.pool = kept;
            for candidate in &gone {
                if let Some(track) = &candidate.track {
                    state.order.retain(|id| id != &track.track_id);
                    state.seen.remove(&track.track_id);
                }
            }
            gone
        };
        for candidate in &removed {
            let _ = std::fs::remove_file(&candidate.path);
        }
        if !removed.is_empty() {
            self.save_playlist();
            self.consumed.notify_one();
            tracing::info!("Out of the queue: {} tracks of {source}", removed.len());
        }
        removed.len()
    }

    /// Play a track of the queue next, as soon as a blend can start.
    pub fn queue_play(&self, track_id: &str) -> Result<(), RadioError> {
        let mut ids = self.queue_ids();
        let Some(at) = ids.iter().position(|id| id == track_id) else {
            return Err(RadioError::UnknownQueueTrack(track_id.to_owned()));
        };
        // Refused before anything is changed if a blend is under way.
        self.request_skip()?;
        ids.remove(at);
        ids.insert(0, track_id.to_owned());
        self.set_order(ids)
    }

    /// Pick the genres. The queue follows them once it is rebuilt.
    pub fn set_wanted_genres(&self, genres: Vec<String>) {
        let genres = unique(genres);
        self.st().wanted_genres = genres.clone();
        self.remember(SETTING_GENRES, &json!(genres));
    }

    /// Pick the genres never to play, in force like the wanted ones.
    pub fn set_blocked_genres(&self, genres: Vec<String>) {
        let genres = unique(genres);
        self.st().wanted_blocked = genres.clone();
        self.remember(SETTING_BLOCKED, &json!(genres));
    }

    /// Take back the tracks of the last run that were loaded and waiting. Their files are in the working directory and
    /// their analyses in the cache, so nothing is downloaded or analyzed again. Files that are not of them (a download
    /// that was cut short, the track that was playing) are removed. Returns how many tracks came back.
    pub fn restore_playlist(&self) -> usize {
        if !self.keep_playlist || self.cache.is_none() {
            return 0;
        }
        let saved = self.setting(SETTING_PLAYLIST);
        for entry in saved
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(candidate) = self.candidate_back(entry) else {
                continue;
            };
            let wanted = {
                let state = self.st();
                self.wanted(&state, &candidate)
            };
            if wanted {
                let mut state = self.st();
                if let Some(track) = &candidate.track {
                    state.seen.insert(track.track_id.clone());
                }
                state.pool.push(candidate);
            } else {
                tracing::info!("Playlist: {} is not wanted now", candidate.title);
                let _ = std::fs::remove_file(&candidate.path);
            }
        }
        let waiting: HashSet<_> = self.st().pool.iter().map(|c| c.path.clone()).collect();
        if let Ok(entries) = std::fs::read_dir(&self.workdir) {
            for entry in entries.flatten() {
                let path = entry.path();
                // Only audio: the directory may be somebody's, and a mistake in the option must not take the cache or the
                // token with it.
                let audio = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| AUDIO_SUFFIXES.contains(&e));
                if audio && !waiting.contains(&path) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        let order = self.setting(SETTING_ORDER);
        let waiting_ids: HashSet<String> = self
            .st()
            .pool
            .iter()
            .filter_map(|c| c.track.as_ref().map(|t| t.track_id.clone()))
            .collect();
        let order: Vec<String> = order
            .as_ref()
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .filter(|id| waiting_ids.contains(*id))
            .map(str::to_owned)
            .collect();
        self.st().order = order;
        self.save_playlist();
        let count = self.st().pool.len();
        tracing::info!("Playlist of the last run: {count} tracks");
        count
    }

    /// Make a candidate of what was kept of it; `None` if it can't be.
    fn candidate_back(&self, entry: &Value) -> Option<Candidate> {
        self.cache.as_ref()?;
        let parsed = (|| -> Option<(Track, std::path::PathBuf, String, String)> {
            let track: Track = serde_json::from_value(entry.get("track")?.clone()).ok()?;
            let file = Path::new(entry.get("file")?.as_str()?)
                .file_name()?
                .to_owned();
            Some((
                track,
                self.workdir.join(file),
                entry.get("title")?.as_str()?.to_owned(),
                entry.get("mood")?.as_str()?.to_owned(),
            ))
        })();
        let Some((track, path, title, mood)) = parsed else {
            tracing::warn!("Ignoring a damaged entry of the playlist");
            return None;
        };
        let analysis = self.cache()?.get_analysis(&track.track_id, &track.codec);
        let usable = path.is_file() && std::fs::metadata(&path).is_ok_and(|m| m.len() > 0);
        let Some(analysis) = analysis.filter(|_| usable) else {
            tracing::info!("Playlist: {title} is gone, left out");
            let _ = std::fs::remove_file(&path);
            return None;
        };
        Some(Candidate {
            title,
            path,
            analysis,
            artists: track.artists.iter().cloned().collect(),
            genre: track.genre.clone(),
            track: Some(track),
            mood,
        })
    }

    /// What the cache keeps under a key (for the tests and the API).
    pub fn setting(&self, key: &str) -> Option<Value> {
        self.cache()?.get_setting(key)
    }

    /// The order of the tracks put in place by hand.
    pub fn order(&self) -> Vec<String> {
        self.st().order.clone()
    }
}
