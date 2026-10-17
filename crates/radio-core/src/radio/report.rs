//! What the radio tells about itself: the queue, the forecast, the settings and the statistics. Task 3.1.8 of
//! docs/rust-port-radio.md, ported from `queue`, `upcoming`, `moods_info`, `language_info`, `genres_info`,
//! `recent_blends`, `transition_stats`, `key_pair_stats`.

use super::{KNOWN_GENRES, LANGUAGES, MAX_LISTED, Radio, TrackInfo};
use crate::{
    mix::{
        choose::{self, ALL_MOODS},
        key,
    },
    pipeline::round_to,
};
use serde_json::{Value, json};
use std::collections::HashSet;

impl Radio {
    /// The track decoded to come next, then a forecast of the pool. The forecast chooses the way the radio does, one track
    /// after the other, but the radio decides only ~40 seconds before each blend, by when the pool has changed.
    pub fn upcoming(&self, limit: usize) -> Vec<TrackInfo> {
        let state = self.st();
        let mut found: Vec<TrackInfo> = Vec::new();
        let last = state.next.as_ref().or(state.now.as_ref());
        let mut current = last.map(|d| d.analysis.clone());
        if let (Some(next), true) = (&state.next, limit > 0) {
            let pinned = next.track.as_ref().is_some_and(|t| state.order.contains(&t.track_id));
            found.push(TrackInfo::of(&next.title, &next.analysis, next.track.as_ref(), &next.mood, true, pinned));
        }
        let mut pool: Vec<_> = state.pool.iter().filter(|c| self.wanted(&state, c)).cloned().collect();
        let mut history: Vec<HashSet<String>> = state.history.iter().cloned().collect();
        let mut position = state.position;
        let keep = |history: &mut Vec<HashSet<String>>, artists: &HashSet<String>| {
            history.push(artists.clone());
            let extra = history.len().saturating_sub(choose::REPEAT_WINDOW);
            history.drain(..extra);
        };
        // What was put in place by hand comes in that order.
        let pinned: Vec<_> = state.order.iter().filter_map(|id| pool.iter().find(|c| c.track.as_ref().is_some_and(|t| &t.track_id == id)).cloned()).collect();
        for candidate in pinned {
            if found.len() >= limit {
                break;
            }
            if let Some(at) = pool.iter().position(|c| *c == candidate) {
                pool.remove(at);
            }
            found.push(TrackInfo::of(&candidate.title, &candidate.analysis, candidate.track.as_ref(), &candidate.mood, true, true));
            keep(&mut history, &candidate.artists);
            position += 1;
            current = Some(candidate.analysis.clone());
        }
        while found.len() < limit && !pool.is_empty() {
            let entries: Vec<choose::Entry> =
                pool.iter().map(|c| choose::Entry { analysis: c.analysis.clone(), artists: c.artists.clone(), mood: c.mood.clone() }).collect();
            let index = choose::choose_next(current.as_ref(), &entries, &history, position);
            let playing = entries[index].analysis.clone();
            let chosen = pool.remove(index);
            found.push(TrackInfo::of(&chosen.title, &playing, chosen.track.as_ref(), &chosen.mood, false, false));
            current = Some(playing);
            keep(&mut history, &chosen.artists);
            position += 1;
        }
        found
    }

    /// The ids of the tracks the API shows as coming.
    pub(crate) fn queue_ids(&self) -> Vec<String> {
        self.upcoming(MAX_LISTED).into_iter().filter(|t| !t.track_id.is_empty()).map(|t| t.track_id).collect()
    }

    /// What was played, what plays now and what comes next.
    pub fn queue(&self, before: usize, after: usize) -> Value {
        let (before, after) = (before.min(MAX_LISTED), after.min(MAX_LISTED));
        let next = self.upcoming(after);
        let state = self.st();
        let now = state.now.as_ref().map(|now| {
            let info = TrackInfo::of(&now.title, &now.analysis, now.track.as_ref(), &now.mood, true, false);
            let mut object = serde_json::to_value(info).expect("track info is JSON");
            object["elapsed_seconds"] = round_to(self.heard(&state), 1).into();
            object
        });
        let previous: Vec<&TrackInfo> = if before == 0 { Vec::new() } else { state.before.iter().rev().take(before).rev().collect() };
        let mut result = json!({
            "previous": previous,
            "now": now,
            "next": next,
        });
        drop(state);
        let object = result.as_object_mut().expect("an object");
        object.insert("moods".to_owned(), self.moods_info());
        object.insert("language".to_owned(), self.language_info());
        for info in [self.skip_info(), self.pause_info()] {
            object.extend(info.as_object().expect("an object").clone());
        }
        result
    }

    pub fn moods_info(&self) -> Value {
        let state = self.st();
        json!({
            "available": ALL_MOODS,
            "selected": state.allowed_moods,
            "station": state.mood,
            // Whether the backend steers its station by the mood; if not, the radio judges the tracks by their sound.
            "steered": self.backend.supports_moods(),
        })
    }

    pub fn language_info(&self) -> Value {
        let state = self.st();
        json!({
            "available": LANGUAGES,
            "selected": state.picked_language,
            "station": state.language,
            // Whether the backend can change its station's language.
            "steered": self.backend.supports_moods(),
        })
    }

    /// Genres to pick from, the ones picked and the ones in force.
    pub fn genres_info(&self) -> Value {
        let known = self.cache().and_then(|c| c.known_genres().ok()).unwrap_or_default();
        let state = self.st();
        let mut available: Vec<String> = known.into_iter().map(|(name, _)| name).collect();
        if available.is_empty() {
            available = KNOWN_GENRES.iter().map(|g| (*g).to_owned()).collect();
        }
        for genre in state.wanted_genres.iter().chain(&state.allowed_genres).chain(&state.wanted_blocked).chain(&state.allowed_blocked) {
            if !available.contains(genre) {
                available.push(genre.clone());
            }
        }
        json!({
            "available": available,
            "selected": state.wanted_genres,
            "applied": state.allowed_genres,
            "blocked": state.wanted_blocked,
            "blocked_applied": state.allowed_blocked,
        })
    }

    /// The last blends, the newest last.
    pub fn recent_blends(&self) -> Vec<Value> {
        self.st().recent.iter().map(|r| serde_json::to_value(r).expect("a report is JSON")).collect()
    }

    /// The transitions of this run and of all runs.
    pub fn transition_stats(&self) -> Value {
        let state = self.st();
        let map = |counter: &super::Counter| -> Value {
            Value::Object(counter.most_common().into_iter().map(|(k, n)| (k, Value::from(n))).collect())
        };
        let by_key = state.transition_keys.most_common();
        let session = json!({
            "total": state.transitions.total(),
            "beat_matched": state.matched_transitions,
            "by_type": map(&state.transitions),
            "by_effect": map(&state.effects),
            "by_key": map(&state.transition_keys),
            "by_quality": key::quality_counts(by_key.iter().map(|(k, n)| (k.as_str(), *n))),
        });
        drop(state);
        let all_time = self.cache().and_then(|c| c.transition_stats().ok()).map(|stats| {
            let map = |pairs: &[(String, i64)]| Value::Object(pairs.iter().map(|(k, n)| (k.clone(), Value::from(*n))).collect());
            json!({
                "total": stats.total,
                "beat_matched": stats.beat_matched,
                "by_type": map(&stats.by_type),
                "by_key": map(&stats.by_key),
                "by_quality": stats.by_quality,
                "by_effect": map(&stats.by_effect),
            })
        });
        json!({"session": session, "all_time": all_time})
    }

    /// Every change of key there was, with how often.
    pub fn key_pair_stats(&self) -> Value {
        let session: Vec<Value> = {
            let state = self.st();
            let mut pairs: Vec<((String, String), i64)> = state.key_pairs.iter().map(|(k, n)| (k.clone(), *n)).collect();
            pairs.sort_by_key(|pair| std::cmp::Reverse(pair.1));
            pairs.into_iter().map(|((now, then), n)| key_pair(&now, &then, n)).collect()
        };
        let all_time = self
            .cache()
            .and_then(|c| c.key_pairs().ok())
            .map(|rows| rows.into_iter().map(|(a, b, n)| key_pair(&a, &b, n)).collect::<Vec<_>>());
        json!({"session": session, "all_time": all_time})
    }
}

/// Describe one change of key; a key that was not found is null.
fn key_pair(now: &str, then: &str, number: i64) -> Value {
    let relation = key::key_relation(key::camelot_key(now), key::camelot_key(then));
    let quality = match key::key_quality(relation) {
        key::KeyQuality::Good => "good",
        key::KeyQuality::Fair => "fair",
        key::KeyQuality::Poor => "poor",
        key::KeyQuality::Unknown => "unknown",
    };
    json!({
        "from": (!now.is_empty()).then_some(now),
        "to": (!then.is_empty()).then_some(then),
        "relation": relation.name(),
        "quality": quality,
        "count": number,
    })
}
