//! Likes: the favourites of the account at the backend, if it has any.

use super::{Radio, RadioError};
use crate::backend::Track;
use serde_json::{Value, json};

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

    /// Whether the backend has likes, and the ids of the tracks played lately that are liked. The backend is asked about a
    /// track once; after that the answer is kept here and changed by `set_like`. A backend that can't be reached is asked
    /// again next time.
    pub async fn likes_info(&self) -> Value {
        if !self.backend.supports_likes() {
            return json!({"supported": false, "liked": Vec::<String>::new()});
        }
        let unknown: Vec<Track> = {
            let state = self.st();
            state
                .played_tracks
                .iter()
                .filter(|t| !state.liked.contains_key(&t.track_id))
                .cloned()
                .collect()
        };
        if !unknown.is_empty() && self.backend.authorized() {
            match self.backend.liked_among(&unknown).await {
                Ok(liked) => {
                    let mut state = self.st();
                    for track in &unknown {
                        state
                            .liked
                            .insert(track.track_id.clone(), liked.contains(&track.track_id));
                    }
                }
                Err(error) => tracing::warn!("The likes could not be read: {error}"),
            }
        }
        let mut liked: Vec<String> = self
            .st()
            .liked
            .iter()
            .filter(|(_, liked)| **liked)
            .map(|(id, _)| id.clone())
            .collect();
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
        self.st().liked.insert(track_id.to_owned(), liked);
        Ok(())
    }
}
