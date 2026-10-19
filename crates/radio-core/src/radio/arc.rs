//! The arc of moods made by hand: the stages the set goes through, from sad to lively, and how long each lasts.

use super::{Radio, RadioError, SETTING_ARC};
use crate::mix::choose::{ALL_MOODS, MAX_STAGE_TRACKS, MAX_STAGES, MIN_STAGES, MoodArc, mood_at};
use serde_json::{Value, json};

/// The arc offered at first: calm, fun, lively, fun, three tracks each.
pub(super) fn default_arc() -> MoodArc {
    MoodArc {
        stages: vec!["calm", "fun", "active", "fun"],
        stage_tracks: 3,
    }
}

impl Radio {
    /// The plan, whether it is in force, and the stage the set is in.
    pub fn arc_info(&self) -> Value {
        let state = self.st();
        json!({
            "enabled": state.arc_on,
            "stages": state.arc.stages,
            "stage_tracks": state.arc.stage_tracks,
            "stage": state.arc.stage_at(state.position),
            "mood": mood_at(Some(&state.arc), state.position),
            "moods": ALL_MOODS,
            "min_stages": MIN_STAGES,
            "max_stages": MAX_STAGES,
            "max_stage_tracks": MAX_STAGE_TRACKS,
        })
    }

    /// Take up the plan kept by the last run.
    pub(super) fn restore_arc(&self) {
        let Some(Value::Object(kept)) = self.setting(SETTING_ARC) else {
            return;
        };
        let stages: Option<Vec<String>> = kept.get("stages").and_then(Value::as_array).map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        });
        let tracks = kept
            .get("stage_tracks")
            .and_then(Value::as_u64)
            .map(|n| n as usize);
        if let (Some(stages), Some(tracks)) = (stages, tracks)
            && let Some(arc) = MoodArc::new(&stages, tracks)
        {
            let mut state = self.st();
            state.arc = arc;
            state.arc_on = kept.get("enabled").and_then(Value::as_bool) == Some(true);
        }
    }

    /// Change the plan (`stages` are mood names, `stage_tracks` how long each lasts) and/or switch it on or off. The queue
    /// follows it once it is rebuilt.
    pub async fn set_arc(
        &self,
        enabled: Option<bool>,
        plan: Option<(Vec<String>, usize)>,
    ) -> Result<(), RadioError> {
        let plan = match plan {
            None => None,
            Some((stages, tracks)) => Some(MoodArc::new(&stages, tracks).ok_or_else(|| {
                RadioError::Invalid(format!(
                    "expected {MIN_STAGES}..{MAX_STAGES} stages of {ALL_MOODS:?} and 1..{MAX_STAGE_TRACKS} tracks for each"
                ))
            })?),
        };
        let on = enabled.unwrap_or_else(|| self.st().arc_on);
        if on && self.st().saved_settings.is_none() {
            // The station is steered along the arc: its settings are put back at the end.
            self.start_mood_arc().await?;
        }
        {
            let mut state = self.st();
            if let Some(plan) = plan {
                state.arc = plan;
            }
            state.arc_on = on;
        }
        let kept = {
            let state = self.st();
            json!({"enabled": state.arc_on, "stages": state.arc.stages, "stage_tracks": state.arc.stage_tracks})
        };
        self.remember(SETTING_ARC, &kept);
        if !on {
            let _ = self.release_settings().await;
        }
        Ok(())
    }
}
