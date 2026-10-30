use super::{Radio, RadioError, TrackInfo};
use crate::{
    mix::{self, SAMPLE_RATE},
    pipeline::{Deck, DeckSource, Playhead, ResumePlayback},
};
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::oneshot;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct PlaybackInfo {
    pub track: TrackInfo,
    pub next: Option<TrackInfo>,
    /// Seconds in the encoded mix and in the original track, respectively.
    pub mix_position: f64,
    pub position: f64,
    pub speed: f64,
    pub acceleration: f64,
    pub valid_until: f64,
}

impl PlaybackInfo {
    pub fn source_at(&self, mix_position: f64) -> f64 {
        let seconds = mix_position - self.mix_position;
        self.position
            + if self.acceleration == 0.0 {
                self.speed * seconds
            } else {
                self.speed * (self.acceleration * seconds).exp_m1() / self.acceleration
            }
    }
}

struct Record {
    at: f64,
    seconds: f64,
    head: Playhead,
}

#[derive(Default)]
pub(crate) struct PlaybackState {
    timeline: VecDeque<Record>,
    retained: VecDeque<Deck>,
    leased: [Option<Deck>; 2],
    lease_versions: [u64; 2],
    lease_owner: String,
    pending: Option<ResumePlayback>,
    acknowledged: Option<oneshot::Sender<PlaybackInfo>>,
    awaiting_first: bool,
}

fn id(deck: &Deck) -> &str {
    deck.track.as_ref().map_or("", |t| t.track_id.as_str())
}

fn info(deck: &Deck) -> TrackInfo {
    TrackInfo::of(
        &deck.title,
        &deck.analysis,
        deck.track.as_ref(),
        &deck.mood,
        true,
        false,
    )
}

fn record_info(record: &Record, at: f64) -> PlaybackInfo {
    let head = record
        .head
        .shifted((at - record.at) * f64::from(SAMPLE_RATE));
    PlaybackInfo {
        track: info(&head.current),
        next: head.following.as_ref().map(info),
        mix_position: at,
        position: head.position / f64::from(SAMPLE_RATE),
        speed: head.speed,
        acceleration: head.acceleration * f64::from(SAMPLE_RATE),
        valid_until: record.at + record.seconds,
    }
}

impl Radio {
    pub fn playback(&self, at: Option<f64>) -> Result<PlaybackInfo, RadioError> {
        if at.is_some_and(|t| !t.is_finite()) {
            return Err(RadioError::Invalid("invalid playback position".into()));
        }
        let mut result = {
            let state = self.st();
            let records = &state.playback.timeline;
            match at {
                Some(at) => {
                    let at = at.max(0.0);
                    records
                        .iter()
                        .rev()
                        .find(|r| r.at <= at && at <= r.at + r.seconds + 0.001)
                        .map(|r| record_info(r, at))
                }
                None => records.back().map(|r| record_info(r, r.at)).or_else(|| {
                    state.now.as_ref().map(|d| PlaybackInfo {
                        track: info(d),
                        next: state.next.as_ref().filter(|n| id(n) != id(d)).map(info),
                        mix_position: 0.0,
                        position: d.analysis.start as f64 / f64::from(SAMPLE_RATE),
                        speed: 1.0,
                        acceleration: 0.0,
                        valid_until: 0.0,
                    })
                }),
            }
            .ok_or_else(|| {
                RadioError::QueueChangeRefused("the playback position is not available yet".into())
            })?
        };
        if result.next.is_none() {
            result.next = self
                .upcoming(2)
                .into_iter()
                .find(|t| t.track_id != result.track.track_id);
        }
        Ok(result)
    }

    pub(super) fn publish_playhead(&self, head: Playhead, at: f64, seconds: f64) {
        let mut state = self.st();
        let playback = &mut state.playback;
        if playback.timeline.back().is_some_and(|r| r.at >= at) {
            playback.timeline.clear();
        }
        let record = Record { at, seconds, head };
        if playback.awaiting_first {
            playback.awaiting_first = false;
            if let Some(ack) = playback.acknowledged.take() {
                let _ = ack.send(record_info(&record, at));
            }
        }
        playback.timeline.push_back(record);
        while playback
            .timeline
            .front()
            .is_some_and(|r| r.at + r.seconds < at - 60.0)
        {
            playback.timeline.pop_front();
        }
    }

    pub(super) fn has_resume(&self) -> bool {
        self.st().playback.pending.is_some()
    }

    pub(super) fn take_resume(&self) -> Option<ResumePlayback> {
        let mut state = self.st();
        let resume = state.playback.pending.take()?;
        state.next = resume.following.clone();
        state.playback.awaiting_first = true;
        Some(resume)
    }

    pub(super) fn position_deck(&self, deck: &Deck, position: i64) {
        let same = self.st().now.as_ref().is_some_and(|d| id(d) == id(deck));
        if !same {
            DeckSource::started(self, deck);
        }
        let mut state = self.st();
        state.now = Some(deck.clone());
        state.now_since = self.clock.monotonic() - position as f64 / f64::from(SAMPLE_RATE);
        if state.paused_at.is_some() {
            state.paused_at = Some(self.clock.monotonic());
        }
    }

    pub(super) async fn deck_for(&self, track_id: &str) -> Result<Deck, RadioError> {
        let candidate = {
            let state = self.st();
            if let Some(deck) = state
                .playback
                .leased
                .iter()
                .flatten()
                .chain(state.playback.retained.iter())
                .chain(state.now.iter())
                .chain(state.next.iter())
                .chain(state.playback.timeline.iter().map(|r| &r.head.current))
                .find(|d| id(d) == track_id)
            {
                return Ok(deck.clone());
            }
            state
                .pool
                .iter()
                .find(|c| c.track.as_ref().is_some_and(|t| t.track_id == track_id))
                .cloned()
                .ok_or_else(|| RadioError::UnknownQueueTrack(track_id.into()))?
        };
        let pcm = mix::decode::decode_file_async(&self.config.ffmpeg, &candidate.path)
            .await
            .map_err(|e| RadioError::Backend(e.to_string()))?;
        let analysis = candidate.analysis.clone();
        let phrases = self.phrase_mix();
        let (pcm, gain_db, phrase) = tokio::task::spawn_blocking(move || {
            let mut pcm = pcm;
            let gain = mix::loudness::normalize(&mut pcm, analysis.loudness_db);
            let phrase = if phrases {
                mix::phrase::analyze_on(&pcm, &analysis).map(Arc::new)
            } else {
                None
            };
            (Arc::<[f32]>::from(pcm), gain, phrase)
        })
        .await
        .map_err(|e| RadioError::Backend(e.to_string()))?;
        let deck = Deck {
            title: candidate.title.as_str().into(),
            pcm,
            analysis: candidate.analysis.clone(),
            gain_db,
            beat_matched: false,
            track: candidate.track.clone(),
            mood: candidate.mood.clone(),
            speed: 1.0,
            generation: self.st().generation,
            source: Some(candidate),
            phrase,
        };
        let mut state = self.st();
        state.playback.retained.retain(|d| id(d) != track_id);
        state.playback.retained.push_back(deck.clone());
        while state.playback.retained.len() > 2 {
            state.playback.retained.pop_front();
        }
        Ok(deck)
    }

    pub async fn leased_track_pcm(
        &self,
        track_id: &str,
        owner: &str,
        slot: usize,
        version: u64,
    ) -> Result<Vec<f32>, RadioError> {
        if owner.is_empty() || owner.len() > 128 || slot > 1 {
            return Err(RadioError::Invalid("invalid DJ deck lease".into()));
        }
        {
            let mut state = self.st();
            let playback = &mut state.playback;
            if playback.lease_owner != owner {
                playback.lease_owner = owner.to_owned();
                playback.leased = [None, None];
                playback.lease_versions = [0, 0];
            }
        }
        let deck = self.deck_for(track_id).await?;
        let mut state = self.st();
        let playback = &mut state.playback;
        if playback.lease_owner == owner && version >= playback.lease_versions[slot] {
            playback.leased[slot] = Some(deck.clone());
            playback.lease_versions[slot] = version;
        }
        drop(state);
        Ok(deck.pcm.to_vec())
    }

    pub fn release_playback_leases(&self, owner: &str) {
        let mut state = self.st();
        if state.playback.lease_owner == owner {
            state.playback.leased = [None, None];
            state.playback.lease_owner.clear();
            state.playback.lease_versions = [0, 0];
        }
    }

    pub fn cancel_playback_handoff(&self) {
        let mut state = self.st();
        state.playback.pending = None;
        state.playback.acknowledged = None;
        state.playback.awaiting_first = false;
        drop(state);
        self.pause();
    }

    pub async fn resume_playback(
        &self,
        track_id: &str,
        position: f64,
        following: Option<&str>,
    ) -> Result<oneshot::Receiver<PlaybackInfo>, RadioError> {
        self.resume_moving_playback(track_id, position, following, 0.0, &[])
            .await
    }

    pub async fn resume_moving_playback(
        &self,
        track_id: &str,
        position: f64,
        following: Option<&str>,
        advance: f64,
        heard: &[String],
    ) -> Result<oneshot::Receiver<PlaybackInfo>, RadioError> {
        let requested_at = self.clock.monotonic();
        if heard.len() > 512 {
            return Err(RadioError::Invalid("too many DJ history entries".into()));
        }
        if !advance.is_finite() || !(0.0..=2.0).contains(&advance) {
            return Err(RadioError::Invalid("invalid playback speed".into()));
        }
        let _handoff = self.playback_lock.lock().await;
        if !position.is_finite() || position < 0.0 {
            return Err(RadioError::Invalid("invalid playback position".into()));
        }
        let busy = {
            let mut state = self.st();
            if state
                .playback
                .acknowledged
                .as_ref()
                .is_some_and(oneshot::Sender::is_closed)
            {
                state.playback.pending = None;
                state.playback.acknowledged = None;
                state.playback.awaiting_first = false;
            }
            state.playback.pending.is_some() || state.playback.acknowledged.is_some()
        };
        if busy {
            return Err(RadioError::QueueChangeRefused(
                "a playback handoff is already in progress".into(),
            ));
        }
        let mut current = self.deck_for(track_id).await?;
        let mut next = match following.filter(|n| *n != track_id) {
            Some(id) => Some(self.deck_for(id).await?),
            None => None,
        };
        let old = self
            .st()
            .next
            .clone()
            .filter(|d| id(d) != track_id && next.as_ref().is_none_or(|n| id(n) != id(d)));
        if let Some(old) = old {
            if heard.iter().any(|played| played == id(&old)) {
                if let Some(source) = old.source {
                    let _ = std::fs::remove_file(source.path);
                }
            } else {
                self.unpick(&old);
            }
        }
        let (tx, rx) = oneshot::channel();
        let picked = {
            let mut state = self.st();
            if state.playback.pending.is_some() || state.playback.acknowledged.is_some() {
                return Err(RadioError::QueueChangeRefused(
                    "a playback handoff is already in progress".into(),
                ));
            }
            state.generation += 1;
            current.generation = state.generation;
            if let Some(next) = &mut next {
                next.generation = state.generation;
                let speed = mix::plan::transition_speed(current.analysis.bpm, &next.analysis);
                next.beat_matched = speed != 0.0;
                next.speed = if speed == 0.0 { 1.0 } else { speed };
            }
            let wanted =
                |id: &str| id == track_id || next.as_ref().is_some_and(|n| self::id(n) == id);
            let mut picked = Vec::new();
            state.pool.retain(|c| {
                if c.track
                    .as_ref()
                    .is_some_and(|t| wanted(&t.track_id) || heard.contains(&t.track_id))
                {
                    picked.push(c.clone());
                    false
                } else {
                    true
                }
            });
            state.order.retain(|id| !wanted(id) && !heard.contains(id));
            state.skip = false;
            state.blending = false;
            state.next = next.clone();
            let position = position + advance * (self.clock.monotonic() - requested_at);
            let position = (position * f64::from(SAMPLE_RATE)).round() as i64;
            let position = position.clamp(
                current.analysis.start,
                current.frames().min(current.analysis.end),
            );
            state.playback.pending = Some(ResumePlayback {
                current,
                following: next,
                position,
            });
            state.playback.acknowledged = Some(tx);
            picked
        };
        for candidate in picked {
            self.mark_played(&candidate);
            if candidate.track.as_ref().is_some_and(|t| {
                heard.contains(&t.track_id)
                    && t.track_id != track_id
                    && Some(t.track_id.as_str()) != following
            }) {
                let _ = std::fs::remove_file(candidate.path);
            }
        }
        self.resume();
        self.consumed.notify_one();
        Ok(rx)
    }
}
