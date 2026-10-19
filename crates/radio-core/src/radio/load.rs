//! Loading tracks into the pool, choosing the next one and making its deck. Tasks 3.1.9 and 3.1.10 of
//! docs/rust-port-radio.md, ported from `_set_mood`, `_load`, `fill`, `keep_filled`, `prepare`, `_choose`, `_pick`,
//! `_replay_oldest`, `_mark_played`, `next_deck`, `_unpick`, `_restart_next`.

use super::{
    Candidate, EXTRA_TRACKS, MAX_EMPTY_BATCHES, MAX_EXTRA_TRACKS, MAX_OFF_GENRE_BATCHES,
    MAX_PLAYED_REMEMBERED, Played, REPLENISH_RETRY_SECONDS, Radio,
};
use crate::{
    backend::{BackendError, Track},
    mix::{self, TrackAnalysis, choose, decode, plan},
    pipeline::{Deck, DeckError},
};
use std::{collections::HashSet, path::PathBuf, time::Duration};

/// Takes the file away when it is dropped, unless it was handed on. A track that was taken out of the pool and is being
/// decoded when the task is stopped is gone with its file, as in the Python radio (`except BaseException: unlink`).
struct UnlinkOnDrop(Option<PathBuf>);

impl UnlinkOnDrop {
    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for UnlinkOnDrop {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Radio {
    pub(crate) async fn set_mood(&self, mood: &str) -> Result<(), BackendError> {
        let (saved, current, language) = {
            let state = self.st();
            (
                state.saved_settings.clone(),
                state.mood.clone(),
                state.language.clone(),
            )
        };
        let Some((_, diversity, _)) = saved else {
            return Ok(());
        };
        if mood == current {
            return Ok(());
        }
        self.backend
            .set_station_settings(&self.station_id(), mood, &diversity, &language)
            .await?;
        self.st().mood = mood.to_owned();
        Ok(())
    }

    /// Download and analyze `count` tracks the radio hasn't had yet. Returns how many were added: 0 when the station only
    /// repeats tracks that were already seen. With `if_empty`, nothing is loaded if somebody else has filled the pool in
    /// the meantime.
    pub(crate) async fn load(
        &self,
        count: usize,
        if_empty: bool,
        mood: &str,
    ) -> Result<usize, BackendError> {
        let _guard = self.load_lock.lock().await;
        self.drop_unwanted();
        if if_empty && !self.st().pool.is_empty() {
            return Ok(1);
        }
        let before = self.st().pool.len();
        let (mut empty_batches, mut off_genre_batches, mut off_mood_batches) = (0, 0, 0);
        let steer = {
            let state = self.st();
            self.backend.supports_moods()
                && (self.config.mood_arc || state.arc_on || !state.allowed_moods.is_empty())
        };
        if steer {
            // Tracks loaded now are played about when the pool has been worked through: aim for the mood wanted then, as
            // far as the moods allowed go.
            let wanted = {
                let state = self.st();
                let mood = if mood.is_empty() {
                    choose::mood_at(
                        state.arc_on.then_some(&state.arc),
                        state.position + state.pool.len() as i64,
                    )
                } else {
                    mood
                };
                let allowed: Vec<&str> = state.allowed_moods.iter().map(String::as_str).collect();
                choose::nearest_mood(mood, &allowed).to_owned()
            };
            self.set_mood(&wanted).await?;
        }
        loop {
            let wanted = count as i64 - (self.st().pool.len() as i64 - before as i64);
            if wanted <= 0 {
                break;
            }
            let exclude: HashSet<String> = {
                let state = self.st();
                state.seen.union(&state.off_genre).cloned().collect()
            };
            let batch = self
                .backend
                .station_batch(&self.station_id(), &exclude)
                .await?;
            let (fresh, wanted_genre) = {
                let mut state = self.st();
                let fresh: Vec<Track> = batch
                    .into_iter()
                    .filter(|t| {
                        !state.seen.contains(&t.track_id) && !state.off_genre.contains(&t.track_id)
                    })
                    .collect();
                let wanted_genre: Vec<Track> = fresh
                    .iter()
                    .filter(|t| Self::genre_allowed(&state, &t.genre))
                    .cloned()
                    .collect();
                let off: Vec<String> = fresh
                    .iter()
                    .filter(|t| !wanted_genre.iter().any(|w| w == *t))
                    .map(|t| t.track_id.clone())
                    .collect();
                state.off_genre.extend(off);
                (fresh, wanted_genre)
            };
            if !fresh.is_empty() && wanted_genre.is_empty() {
                off_genre_batches += 1;
                if off_genre_batches >= MAX_OFF_GENRE_BATCHES {
                    break;
                }
                continue;
            }
            off_genre_batches = 0;
            let fresh: Vec<Track> = wanted_genre.into_iter().take(wanted as usize).collect();
            let (added, rejected) = {
                let mut state = self.st();
                state.seen.extend(fresh.iter().map(|t| t.track_id.clone()));
                (state.pool.len(), state.off_mood)
            };
            futures_util::future::join_all(fresh.into_iter().map(|track| self.prepare(track)))
                .await;
            let (now, off_mood) = {
                let state = self.st();
                (state.pool.len(), state.off_mood)
            };
            if now == added && off_mood != rejected {
                // Good tracks, but not of the moods wanted.
                off_mood_batches += 1;
                if off_mood_batches >= MAX_OFF_GENRE_BATCHES {
                    break;
                }
            } else if now == added {
                empty_batches += 1;
                if empty_batches >= MAX_EMPTY_BATCHES {
                    break;
                }
            } else {
                empty_batches = 0;
            }
        }
        Ok(self.st().pool.len() - before)
    }

    /// Load tracks until the pool has `size` of them, if there are.
    pub async fn fill(&self, size: Option<usize>) -> Result<(), BackendError> {
        let size = size.unwrap_or(self.config.pool_size);
        while self.st().pool.len() < size {
            let mut wanted = size - self.st().pool.len();
            let steered = self.backend.supports_moods() && {
                let state = self.st();
                self.config.mood_arc || state.arc_on || !state.allowed_moods.is_empty()
            };
            if steered {
                // A few at a time: each round has its own mood.
                wanted = wanted.min(EXTRA_TRACKS);
            }
            if self.load(wanted, false, "").await? == 0 {
                tracing::warn!("The station has no new tracks");
                return Ok(());
            }
        }
        Ok(())
    }

    /// Background task: top the pool up whenever tracks are taken.
    pub async fn keep_filled(&self) {
        use futures_util::FutureExt;
        loop {
            // Python clears the event here: a wake-up that came before is not kept.
            let _ = self.consumed.notified().now_or_never();
            self.drop_unwanted();
            if let Err(error) = self.fill(None).await {
                tracing::error!("Could not refill the pool, retry in 30s: {error}");
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            if self.st().pool.len() < self.config.pool_size {
                tokio::time::sleep(Duration::from_secs(REPLENISH_RETRY_SECONDS)).await;
                continue;
            }
            self.consumed.notified().await;
        }
    }

    /// Download one track, analyze it (or take its analysis from the cache) and put it in the pool.
    pub(crate) async fn prepare(&self, track: Track) {
        let slot = self
            .slots
            .acquire()
            .await
            .expect("the semaphore is never closed");
        let title = format!("{} - {}", track.artists.join(", "), track.title);
        let path = self
            .workdir
            .join(format!("{}.{}", track.track_id, track.codec));
        let made: Result<(TrackAnalysis, String), String> = async {
            self.backend
                .download(&track, &path)
                .await
                .map_err(|e| e.to_string())?;
            let analysis = match self.cached_analysis(&track) {
                Some(analysis) => analysis,
                None => {
                    // One at a time: it takes a few times the track in memory.
                    let _turn = self
                        .analyzing
                        .acquire()
                        .await
                        .expect("the semaphore is never closed");
                    let pcm = decode::decode_file_async(&self.config.ffmpeg, &path)
                        .await
                        .map_err(|e| e.to_string())?;
                    let analysis = tokio::task::spawn_blocking(move || mix::analyze(&pcm))
                        .await
                        .map_err(|e| e.to_string())?;
                    self.store_analysis(&track, &title, &analysis);
                    analysis
                }
            };
            self.store_genre(&track);
            let mood = if track.mood.is_empty() {
                choose::mood_of(&analysis).to_owned()
            } else {
                track.mood.clone()
            };
            Ok((analysis, mood))
        }
        .await;
        let (analysis, mood) = match made {
            Ok(made) => made,
            Err(error) => {
                tracing::error!("Skip {title}: {error}");
                let _ = std::fs::remove_file(&path);
                return;
            }
        };
        if !Self::mood_allowed(&self.st(), &mood) {
            tracing::info!("Skip {title}: {mood} is not a mood wanted");
            let _ = std::fs::remove_file(&path);
            self.st().off_mood += 1;
            return;
        }
        drop(slot);
        tracing::info!(
            "Ready: {title} ({:.1} BPM, {:.1} dB, key {})",
            analysis.bpm,
            analysis.loudness_db,
            if analysis.key >= 0 {
                format!(
                    "{} {}",
                    mix::key::camelot_code(analysis.key),
                    mix::key::key_name(analysis.key)
                )
            } else {
                "?".to_owned()
            }
        );
        let genre = track.genre.clone();
        self.st().pool.push(Candidate {
            title,
            path,
            analysis,
            artists: track.artists.iter().cloned().collect(),
            track: Some(track),
            mood,
            genre,
        });
        self.save_playlist();
    }

    fn cached_analysis(&self, track: &Track) -> Option<TrackAnalysis> {
        self.cache()?.get_analysis(&track.track_id, &track.codec)
    }

    fn store_analysis(&self, track: &Track, title: &str, analysis: &TrackAnalysis) {
        if let Some(cache) = self.cache() {
            let _ =
                cache.put_analysis(&track.track_id, &track.codec, title, analysis, &track.genre);
        }
    }

    /// Keep the genre of a track whose analysis was cached before.
    fn store_genre(&self, track: &Track) {
        if let Some(cache) = self.cache() {
            if !track.genre.is_empty() {
                let _ = cache.set_genre(&track.track_id, &track.codec, &track.genre);
            }
        }
    }

    /// The best next track of the pool and what its transition costs.
    fn choose_from_pool(&self, current: Option<&TrackAnalysis>) -> (usize, f64) {
        let state = self.st();
        let entries: Vec<choose::Entry> = state
            .pool
            .iter()
            .map(|c| choose::Entry {
                analysis: c.analysis.clone(),
                artists: c.artists.clone(),
                mood: c.mood.clone(),
            })
            .collect();
        let history: Vec<HashSet<String>> = state.history.iter().cloned().collect();
        let arc = state.arc_on.then_some(&state.arc);
        let index = choose::choose_next_with(arc, current, &entries, &history, state.position);
        let cost = choose::step_cost_with(arc, current, &entries[index], &history, state.position);
        (index, cost)
    }

    /// Take the track to play next out of the pool. If nothing in the pool fits, a few more tracks are loaded to choose
    /// from. If the station has nothing new at all, the track that was played the longest ago is played again.
    pub(crate) async fn pick(
        &self,
        current: Option<&TrackAnalysis>,
    ) -> Result<Candidate, DeckError> {
        let mut extra = 0;
        let index = loop {
            self.drop_unwanted();
            if self.st().pool.is_empty() {
                let loaded = self
                    .load(EXTRA_TRACKS, true, "")
                    .await
                    .map_err(|e| DeckError(e.to_string()))?;
                if loaded == 0 {
                    return self.replay_oldest().await;
                }
            }
            if self.st().pool.is_empty() {
                continue;
            }
            if let Some(pinned) = self.first_pinned() {
                // Put in place by hand: no need to find what fits.
                break pinned;
            }
            let (index, cost) = self.choose_from_pool(current);
            if cost <= choose::SUITABLE_COST || extra >= MAX_EXTRA_TRACKS {
                break index;
            }
            let (title, mood) = {
                let state = self.st();
                (
                    state.pool[index].title.clone(),
                    choose::mood_at(state.arc_on.then_some(&state.arc), state.position),
                )
            };
            tracing::info!("Nothing fits well (best: {title}, cost {cost:.1}), loading more");
            let added = self
                .load(EXTRA_TRACKS.min(MAX_EXTRA_TRACKS - extra), false, mood)
                .await
                .map_err(|e| DeckError(e.to_string()))?;
            if added == 0 {
                break index;
            }
            extra += added;
        };
        let candidate = {
            let mut state = self.st();
            // The pool may have changed while more was loaded: take the best one again if the index is not the same track.
            let index = index.min(state.pool.len() - 1);
            state.pool.remove(index)
        };
        self.save_playlist();
        Ok(candidate)
    }

    /// Download again the track that was played the longest ago.
    async fn replay_oldest(&self) -> Result<Candidate, DeckError> {
        let played: Vec<(String, Played)> = self
            .st()
            .played
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let playing = self.st().playing_id.clone();
        let played_all = &played;
        for (track_id, played) in played_all {
            if *track_id == playing && played_all.len() > 1 {
                continue;
            }
            let path = self
                .workdir
                .join(format!("{track_id}-again.{}", played.track.codec));
            if let Err(error) = self.backend.download(&played.track, &path).await {
                tracing::error!("Cannot repeat {}: {error}", played.title);
                let _ = std::fs::remove_file(&path);
                continue;
            }
            tracing::info!("The station repeats itself, again: {}", played.title);
            return Ok(Candidate {
                title: played.title.clone(),
                path,
                analysis: played.analysis.clone(),
                artists: played.artists.clone(),
                track: Some(played.track.clone()),
                mood: String::new(),
                genre: played.track.genre.clone(),
            });
        }
        Err(DeckError("No tracks to play".to_owned()))
    }

    fn mark_played(&self, candidate: &Candidate) {
        let mut state = self.st();
        state.history.push_back(candidate.artists.clone());
        while state.history.len() > choose::REPEAT_WINDOW {
            state.history.pop_front();
        }
        state.position += 1;
        let Some(track) = &candidate.track else {
            return;
        };
        state.playing_id = track.track_id.clone();
        drop(state);
        if let Some(cache) = self.cache() {
            let _ = cache.mark_played(&track.track_id, &track.codec);
        }
        let mut state = self.st();
        let played = Played {
            track: track.clone(),
            title: candidate.title.clone(),
            analysis: candidate.analysis.clone(),
            artists: candidate.artists.clone(),
        };
        // move_to_end: a track played again is the newest.
        state.played.shift_remove(&track.track_id);
        state.played.insert(track.track_id.clone(), played);
        while state.played.len() > MAX_PLAYED_REMEMBERED {
            state.played.shift_remove_index(0);
        }
    }

    /// Choose, tempo-match and decode the track to play after `current`.
    pub async fn next_deck(&self, current: Option<&TrackAnalysis>) -> Result<Deck, DeckError> {
        loop {
            let generation = self.st().generation;
            let candidate = self.pick(current).await?;
            self.mark_played(&candidate);
            self.consumed.notify_one();

            let speed = current.map_or(0.0, |c| plan::transition_speed(c.bpm, &candidate.analysis));
            let mut guard = UnlinkOnDrop(Some(candidate.path.clone()));
            let pcm = decode::decode_file_async(&self.config.ffmpeg, &candidate.path)
                .await
                .map_err(|e| DeckError(e.to_string()))?;
            guard.disarm();
            let deck = Deck {
                title: candidate.title.as_str().into(),
                pcm: pcm.into(),
                analysis: candidate.analysis.clone(),
                beat_matched: speed != 0.0,
                track: candidate.track.clone(),
                mood: candidate.mood.clone(),
                speed: if speed != 0.0 { speed } else { 1.0 },
                source: Some(candidate),
                generation,
            };
            if generation != self.st().generation {
                // The queue was rebuilt while the track was decoded.
                self.unpick(&deck);
                continue;
            }
            tracing::info!(
                "Next: {} ({:.1} BPM, key {}{})",
                deck.title,
                deck.analysis.bpm,
                if deck.analysis.key >= 0 {
                    mix::key::camelot_code(deck.analysis.key)
                } else {
                    "?".to_owned()
                },
                if speed != 0.0 {
                    format!(", tempo glides by {:.1}%", (speed - 1.0).abs() * 100.0)
                } else {
                    String::new()
                }
            );
            self.st().next = Some(deck.clone());
            self.save_playlist();
            return Ok(deck);
        }
    }

    /// Take back the choice of a deck that has not been heard yet.
    pub(crate) fn unpick(&self, deck: &Deck) {
        let Some(source) = &deck.source else { return };
        let mut state = self.st();
        if state.history.back() == Some(&source.artists) {
            state.history.pop_back();
        }
        state.position = (state.position - 1).max(0);
        if let Some(track) = &source.track {
            state.played.shift_remove(&track.track_id);
            state.playing_id = state
                .now
                .as_ref()
                .and_then(|d| d.track.as_ref())
                .map_or_else(String::new, |t| t.track_id.clone());
        }
        if source.path.exists() {
            state.pool.push(source.clone());
            drop(state);
            self.save_playlist();
        }
    }

    /// Choose the next track anew: the one decoded goes back to the pool.
    pub(crate) fn restart_next(&self) {
        let deck = {
            let mut state = self.st();
            state.generation += 1;
            state.next.take()
        };
        if let Some(deck) = deck {
            self.unpick(&deck);
        }
        self.consumed.notify_one();
    }

    /// The index in the pool of the first track that was put in place by hand.
    pub(crate) fn first_pinned(&self) -> Option<usize> {
        let state = self.st();
        state.order.iter().find_map(|id| {
            state
                .pool
                .iter()
                .position(|c| c.track.as_ref().is_some_and(|t| &t.track_id == id))
        })
    }
}
