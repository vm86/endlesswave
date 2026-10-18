//! The pool of loaded tracks: which may be played, what is let go, and the playlist kept for a restart. Tasks 3.1.4 and
//! 3.1.5 of docs/rust-port-radio.md, ported from `_wanted`, `_reachable`, `_mood_allowed`, `_genre_allowed`,
//! `_drop_unwanted`, `backends_changed`, `_save_playlist`.

use super::{Candidate, Radio, SETTING_ORDER, SETTING_PLAYLIST, State};
use serde_json::{Value, json};

impl Radio {
    pub(crate) fn mood_allowed(state: &State, mood: &str) -> bool {
        state.allowed_moods.is_empty() || state.allowed_moods.iter().any(|m| m == mood)
    }

    pub(crate) fn genre_allowed(state: &State, genre: &str) -> bool {
        if state.allowed_blocked.iter().any(|g| g == genre) {
            return false;
        }
        state.allowed_genres.is_empty() || state.allowed_genres.iter().any(|g| g == genre)
    }

    /// Whether the audio of a pool track can still be read. A track that was downloaded is a file of its own. One that is
    /// only linked (a folder's) stays in its backend's storage, so it goes with the backend when that is turned off or
    /// can't be reached.
    pub(crate) fn reachable(&self, candidate: &Candidate) -> bool {
        if !candidate.path.exists() {
            return false; // gone, or a link to what is gone
        }
        let Some(track) = &candidate.track else {
            return true;
        };
        if !candidate.path.is_symlink() {
            return true;
        }
        !self.backend.unavailable().contains(&track.source)
    }

    /// Whether a pool track may be played: its mood and genre are allowed, and its audio is there to be read.
    pub(crate) fn wanted(&self, state: &State, candidate: &Candidate) -> bool {
        Self::mood_allowed(state, &candidate.mood)
            && Self::genre_allowed(state, &candidate.genre)
            && self.reachable(candidate)
    }

    /// Forget the pool tracks of other moods or genres, or out of reach.
    pub(crate) fn drop_unwanted(&self) {
        let decoded = self.st().next.as_ref().and_then(|d| d.source.clone());
        if let Some(decoded) = decoded {
            let blending = self.st().blending;
            if !blending && !self.reachable(&decoded) {
                // Chosen and decoded already, but of a backend that went: it goes back to the pool, to be given up like
                // the others.
                self.restart_next();
            }
        }
        let pool = self.st().pool.clone();
        for candidate in pool {
            let wanted = {
                let state = self.st();
                self.wanted(&state, &candidate)
            };
            if wanted {
                continue;
            }
            {
                let mut state = self.st();
                if let Some(at) = state.pool.iter().position(|c| *c == candidate) {
                    state.pool.remove(at);
                }
            }
            let _ = std::fs::remove_file(&candidate.path);
            // As in the Python radio, the file is gone by now, so the track is never "reachable" here: whatever the reason it
            // was dropped for, it is not counted as seen, and may be offered again.
            if !self.reachable(&candidate) {
                if let Some(track) = &candidate.track {
                    self.st().seen.remove(&track.track_id);
                }
            }
            self.save_playlist();
        }
    }

    /// Let go of the tracks of a backend that went, and look for more.
    pub fn backends_changed(&self) {
        self.drop_unwanted();
        self.consumed.notify_one();
    }

    /// Keep what is loaded and waiting, for a restart to take up.
    pub(crate) fn save_playlist(&self) {
        if !self.keep_playlist {
            return;
        }
        let (entries, order) = {
            let state = self.st();
            let waiting = state.next.as_ref().and_then(|d| d.source.clone());
            let entries: Vec<Value> = waiting
                .iter()
                .chain(state.pool.iter())
                .filter(|c| c.track.is_some() && c.path.exists())
                .map(candidate_json)
                .collect();
            (entries, state.order.clone())
        };
        self.remember(SETTING_PLAYLIST, &Value::Array(entries));
        self.remember(SETTING_ORDER, &json!(order));
    }
}

/// Keep a candidate (its analysis is in the cache) as JSON.
fn candidate_json(candidate: &Candidate) -> Value {
    json!({
        "title": candidate.title,
        "file": candidate.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "mood": candidate.mood,
        "track": candidate.track.as_ref().map(|t| serde_json::to_value(t).expect("a track is JSON")),
    })
}
