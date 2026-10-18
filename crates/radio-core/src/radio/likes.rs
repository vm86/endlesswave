//! Likes: the favourites of the account at the backend, if it has any.

use super::{Radio, RadioError};
use crate::backend::Track;
use serde_json::{Value, json};
use std::collections::HashSet;

impl Radio {
    /// A track the radio knows (now, next, in the pool or played lately) by its id.
    fn known_track(&self, track_id: &str) -> Option<Track> {
        let state = self.st();
        let same = |t: &&Track| t.track_id == track_id;
        state
            .now
            .iter()
            .chain(state.next.iter())
            .filter_map(|d| d.track.as_ref())
            .chain(state.pool.iter().filter_map(|c| c.track.as_ref()))
            .chain(state.played_tracks.iter())
            .find(same)
            .cloned()
    }

    /// Whether the backend has likes, and the ids of the tracks that are liked. The backend is asked once; after that
    /// the likes are kept here and changed by `set_like`. A backend that can't be reached is asked again next time.
    pub async fn likes_info(&self) -> Value {
        if !self.backend.supports_likes() {
            return json!({"supported": false, "liked": Vec::<String>::new()});
        }
        if self.st().liked.is_none() && self.backend.authorized() {
            match self.backend.liked_track_ids().await {
                Ok(ids) => self.st().liked = Some(ids),
                Err(error) => tracing::warn!("The likes could not be read: {error}"),
            }
        }
        let mut liked: Vec<String> = self
            .st()
            .liked
            .as_ref()
            .map(|set: &HashSet<String>| set.iter().cloned().collect())
            .unwrap_or_default();
        liked.sort();
        json!({"supported": true, "liked": liked})
    }

    /// Like the track, or take the like back.
    pub async fn set_like(&self, track_id: &str, liked: bool) -> Result<(), RadioError> {
        if !self.backend.supports_likes() {
            return Err(RadioError::Invalid("the backend has no likes".to_owned()));
        }
        let Some(track) = self.known_track(track_id) else {
            return Err(RadioError::UnknownQueueTrack(track_id.to_owned()));
        };
        self.backend
            .set_like(&track, liked)
            .await
            .map_err(|e| RadioError::Backend(e.to_string()))?;
        let mut state = self.st();
        let set = state.liked.get_or_insert_with(HashSet::new);
        if liked {
            set.insert(track_id.to_owned());
        } else {
            set.remove(track_id);
        }
        Ok(())
    }
}
