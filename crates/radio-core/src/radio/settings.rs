//! The settings of the station on the account (the moods, the language), and putting them back. Task 3.1.7 of
//! docs/rust-port-radio.md, ported from `start_mood_arc`, `restore_settings`, `restore_state`, `set_allowed_moods`,
//! `_release_settings`, `set_language`, `_give_up_the_pool`, `authorize`, `auth_info`.
//!
//! A backend that steers its station by the mood and the language (Yandex does) has those settings changed on the account
//! while the radio runs; they are put back when it stops, and kept in the cache meanwhile, so that if the radio is
//! killed the next run puts them back first.

use super::{
    LANGUAGE_KEEP_TRACKS, LANGUAGES, Radio, RadioError, SETTING_BLOCKED, SETTING_GENRES,
    SETTING_LANGUAGE, SETTING_MOODS, SETTING_STATION,
};
use crate::{backend::BackendError, mix::choose::ALL_MOODS};
use serde_json::{Value, json};
use std::collections::HashMap;

fn backend_error(error: BackendError) -> RadioError {
    RadioError::Backend(error.to_string())
}

impl Radio {
    /// Remember the station's settings, to put them back at the end. They are kept on disk as well, for as long as the
    /// account has something else: if the radio is killed, the next run puts them back.
    pub async fn start_mood_arc(&self) -> Result<(), RadioError> {
        if !self.backend.supports_moods() {
            return Ok(());
        }
        let saved = self
            .backend
            .station_settings(&self.station_id())
            .await
            .map_err(backend_error)?;
        {
            let mut state = self.st();
            state.mood = saved.0.clone();
            state.language = saved.2.clone();
            state.saved_settings = Some(saved.clone());
        }
        self.remember(
            SETTING_STATION,
            &json!({"station": self.station_id(), "settings": [saved.0, saved.1, saved.2]}),
        );
        Ok(())
    }

    /// Put the station's settings back as they were.
    pub async fn restore_settings(&self) -> Result<(), RadioError> {
        let (saved, mood, language) = {
            let state = self.st();
            (
                state.saved_settings.clone(),
                state.mood.clone(),
                state.language.clone(),
            )
        };
        let Some(saved) = saved else { return Ok(()) };
        if (mood.as_str(), language.as_str()) != (saved.0.as_str(), saved.2.as_str()) {
            self.backend
                .set_station_settings(&self.station_id(), &saved.0, &saved.1, &saved.2)
                .await
                .map_err(backend_error)?;
            let mut state = self.st();
            state.mood = saved.0;
            state.language = saved.2;
        }
        self.forget(SETTING_STATION);
        Ok(())
    }

    /// Take up where the last run left off. If it was killed with the station's settings changed, they are put back
    /// first; then the moods and the genres that were picked are picked again. Needs the app to be authorized.
    pub async fn restore_state(&self) -> Result<(), RadioError> {
        if self.cache.is_none() {
            return Ok(());
        }
        if let Some(Value::Object(left)) = self.setting(SETTING_STATION) {
            let parsed = (|| {
                let station = left.get("station")?;
                let station = station
                    .as_str()
                    .map_or_else(|| station.to_string(), str::to_owned);
                let settings: Vec<String> = left
                    .get("settings")?
                    .as_array()?
                    .iter()
                    .map(|v| v.as_str().map(str::to_owned))
                    .collect::<Option<_>>()?;
                let [mood, diversity, language]: [String; 3] = settings.try_into().ok()?;
                Some((station, mood, diversity, language))
            })();
            match parsed {
                None => {
                    tracing::warn!("Ignoring the station settings left on disk");
                    self.forget(SETTING_STATION);
                }
                Some((station, mood, diversity, language)) => match self
                    .backend
                    .set_station_settings(&station, &mood, &diversity, &language)
                    .await
                {
                    // Left for the next run, which may be able to.
                    Err(error) => {
                        tracing::error!("Could not put the station's settings back: {error}")
                    }
                    Ok(()) => {
                        tracing::info!("The station's settings put back (the last run left them)");
                        self.forget(SETTING_STATION);
                    }
                },
            }
        }
        let strings = |key: &str| -> Option<Vec<String>> {
            Some(
                self.setting(key)?
                    .as_array()?
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect(),
            )
        };
        if let Some(genres) = strings(SETTING_GENRES) {
            // Nothing is queued yet, so they are in force at once.
            let mut state = self.st();
            state.wanted_genres = genres.clone();
            state.allowed_genres = genres;
        }
        if let Some(blocked) = strings(SETTING_BLOCKED) {
            let mut state = self.st();
            state.wanted_blocked = blocked.clone();
            state.allowed_blocked = blocked;
        }
        if let Some(moods) = strings(SETTING_MOODS).filter(|m| !m.is_empty()) {
            self.set_allowed_moods(moods).await?;
        }
        if let Some(Value::String(language)) = self.setting(SETTING_LANGUAGE) {
            if LANGUAGES.contains(&language.as_str()) {
                self.set_language(Some(&language)).await?;
            }
        }
        self.restore_arc();
        if self.st().arc_on && self.st().saved_settings.is_none() {
            self.start_mood_arc().await?;
        }
        Ok(())
    }

    /// Play only tracks of these moods, or any with none given. The station's mood setting is kept on the account: it is
    /// changed for as long as the radio runs and put back when it stops.
    pub async fn set_allowed_moods(&self, moods: Vec<String>) -> Result<(), RadioError> {
        let wanted: Vec<String> = ALL_MOODS
            .iter()
            .filter(|m| moods.iter().any(|c| c == *m))
            .map(|m| (*m).to_owned())
            .collect();
        if !wanted.is_empty() && self.st().saved_settings.is_none() {
            self.start_mood_arc().await?;
        }
        self.st().allowed_moods = wanted.clone();
        self.remember(SETTING_MOODS, &json!(wanted));
        self.release_settings().await?;
        self.drop_unwanted();
        tracing::info!(
            "Moods played: {}",
            if wanted.is_empty() {
                "any".to_owned()
            } else {
                wanted.join(", ")
            }
        );
        // Wake up the refill: the pool may be short of tracks now.
        self.consumed.notify_one();
        Ok(())
    }

    /// Put the station's settings back once nothing here steers them.
    pub(super) async fn release_settings(&self) -> Result<(), RadioError> {
        let free = {
            let state = self.st();
            state.saved_settings.is_some()
                && state.allowed_moods.is_empty()
                && !self.config.mood_arc
                && !state.arc_on
                && state.picked_language.is_none()
        };
        if free {
            self.restore_settings().await?;
            self.st().saved_settings = None;
        }
        Ok(())
    }

    /// Play music of a language, or the account's own with `None`. Like the moods, it is a setting of the station, kept on
    /// the account: it is changed for as long as the radio runs and put back when it stops. The tracks loaded already are
    /// not of a known language, so most of them are given up (the ones needed next stay) and the station is asked for
    /// tracks anew.
    pub async fn set_language(&self, language: Option<&str>) -> Result<(), RadioError> {
        if let Some(language) = language {
            if !LANGUAGES.contains(&language) {
                return Err(RadioError::Invalid(format!(
                    "unknown language '{language}'"
                )));
            }
        }
        if language.is_some() && self.st().saved_settings.is_none() {
            self.start_mood_arc().await?;
        }
        let Some(saved) = self.st().saved_settings.clone() else {
            return Ok(()); /* nothing was changed: nothing to put back */
        };
        let target = language.map_or_else(|| saved.2.clone(), str::to_owned);
        let (current, mood) = {
            let state = self.st();
            (state.language.clone(), state.mood.clone())
        };
        if target != current {
            self.backend
                .set_station_settings(&self.station_id(), &mood, &saved.1, &target)
                .await
                .map_err(backend_error)?;
            self.st().language = target;
            self.give_up_the_pool();
        }
        self.st().picked_language = language.map(str::to_owned);
        match language {
            None => self.forget(SETTING_LANGUAGE),
            Some(language) => self.remember(SETTING_LANGUAGE, &json!(language)),
        }
        self.release_settings().await?;
        tracing::info!(
            "Language of the music: {}",
            language.unwrap_or("the account's own")
        );
        Ok(())
    }

    /// Drop the loaded tracks but the few that are needed next.
    fn give_up_the_pool(&self) {
        let gone: Vec<_> = {
            let mut state = self.st();
            let keep = LANGUAGE_KEEP_TRACKS.min(state.pool.len());
            state.pool.split_off(keep)
        };
        for candidate in gone {
            let _ = std::fs::remove_file(&candidate.path);
        }
        self.save_playlist();
        // Wake up the refill: the pool is short of tracks now.
        self.consumed.notify_one();
    }

    /// Sign in to the backend with what the user typed.
    pub async fn authorize(&self, values: &HashMap<String, String>) -> Result<(), RadioError> {
        self.backend
            .authorize(values)
            .await
            .map_err(|e| RadioError::Backend(e.to_string()))
    }

    /// What to do to authorize the app, while it isn't.
    pub fn auth_info(&self) -> Value {
        let authorized = self.backend.authorized();
        let prompt = self.backend.auth_prompt();
        json!({
            "authorized": authorized,
            "url": if authorized { None } else { prompt.url },
            "code": if authorized { None } else { prompt.code },
            "fields": if authorized { Vec::new() } else { prompt.fields },
        })
    }

    /// The moods the radio plays now (empty is any).
    pub fn allowed_moods(&self) -> Vec<String> {
        self.st().allowed_moods.clone()
    }

    /// The mood setting the station has now.
    pub fn mood(&self) -> String {
        self.st().mood.clone()
    }

    /// The station's settings put aside, `None` before that.
    pub fn mood_arc_saved(&self) -> Option<(String, String, String)> {
        self.st().saved_settings.clone()
    }
}
