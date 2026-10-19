//! Morning, day, evening: a cycle plays the set along a plan of the tempo of its own.
//!
//! - Morning starts quiet, about 60 BPM, and goes up to 120 BPM stage by stage.
//! - Day plays the arc of moods, within a range of BPM.
//! - Evening plays quiet music only, and goes down from 100–120 BPM to 40–60 BPM.
//!
//! Each cycle has its plan, which the listener can change; the plan of the morning and the evening is a point for each
//! stage (its BPM), the length of a stage, and how far from the point a track may be.

use super::{Radio, RadioError, SETTING_CYCLE, State};
use crate::mix::choose::{BpmPlan, MAX_BPM, MIN_BPM, Steer};
use serde_json::{Value, json};

pub const MIN_STAGES: usize = 2;
pub const MAX_STAGES: usize = 12;
pub const MAX_STAGE_TRACKS: usize = 10;
pub const MIN_TOLERANCE: f64 = 3.0;
pub const MAX_TOLERANCE: f64 = 30.0;
/// The least the range of the day may be.
pub const MIN_DAY_SPAN: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cycle {
    Off,
    Morning,
    Day,
    Evening,
}

impl Cycle {
    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Morning => "morning",
            Self::Day => "day",
            Self::Evening => "evening",
        }
    }

    fn of(name: &str) -> Option<Self> {
        [Self::Off, Self::Morning, Self::Day, Self::Evening]
            .into_iter()
            .find(|c| c.name() == name)
    }
}

/// The plan of the morning or the evening.
#[derive(Debug, Clone, PartialEq)]
pub struct Ramp {
    pub stages: Vec<f64>,
    pub stage_tracks: usize,
    pub tolerance: f64,
}

impl Ramp {
    fn plan(&self) -> BpmPlan {
        BpmPlan {
            stages: self.stages.clone(),
            stage_tracks: self.stage_tracks,
            tolerance: self.tolerance,
            replaces_arc: true,
        }
    }

    fn json(&self) -> Value {
        json!({"stages": self.stages, "stage_tracks": self.stage_tracks, "tolerance": self.tolerance})
    }

    /// The ramp described by `value`, or why not.
    fn parse(value: &Value) -> Result<Self, String> {
        let stages: Vec<f64> = value
            .get("stages")
            .and_then(Value::as_array)
            .ok_or("expected \"stages\": [BPM, ...]")?
            .iter()
            .map(|v| v.as_f64().filter(|b| (MIN_BPM..=MAX_BPM).contains(b)))
            .collect::<Option<_>>()
            .ok_or_else(|| format!("each stage is a BPM of {MIN_BPM}..{MAX_BPM}"))?;
        if !(MIN_STAGES..=MAX_STAGES).contains(&stages.len()) {
            return Err(format!("{MIN_STAGES}..{MAX_STAGES} stages"));
        }
        let stage_tracks = value
            .get("stage_tracks")
            .and_then(Value::as_u64)
            .filter(|n| (1..=MAX_STAGE_TRACKS as u64).contains(n))
            .ok_or_else(|| format!("\"stage_tracks\" is 1..{MAX_STAGE_TRACKS}"))?
            as usize;
        let tolerance = value
            .get("tolerance")
            .and_then(Value::as_f64)
            .filter(|t| (MIN_TOLERANCE..=MAX_TOLERANCE).contains(t))
            .ok_or_else(|| format!("\"tolerance\" is {MIN_TOLERANCE}..{MAX_TOLERANCE} BPM"))?;
        Ok(Self {
            stages,
            stage_tracks,
            tolerance,
        })
    }
}

/// The plans of the three cycles, and which is on.
#[derive(Debug, Clone, PartialEq)]
pub struct Cycles {
    pub active: Cycle,
    pub morning: Ramp,
    /// The range of the day, in BPM.
    pub day: (f64, f64),
    pub evening: Ramp,
}

impl Default for Cycles {
    fn default() -> Self {
        Self {
            active: Cycle::Off,
            morning: Ramp {
                stages: vec![60.0, 75.0, 90.0, 105.0, 120.0],
                stage_tracks: 3,
                tolerance: 8.0,
            },
            day: (95.0, 135.0),
            evening: Ramp {
                stages: vec![110.0, 95.0, 80.0, 65.0, 50.0],
                stage_tracks: 3,
                tolerance: 10.0,
            },
        }
    }
}

/// What steers the choice now: the cycle's plan of the tempo and, for the day, the arc of moods.
pub(super) fn steer(state: &State) -> Steer {
    let cycles = &state.cycles;
    match cycles.active {
        Cycle::Off => Steer {
            arc: state.arc_on.then(|| state.arc.clone()),
            bpm: None,
        },
        Cycle::Morning => Steer {
            arc: None,
            bpm: Some(cycles.morning.plan()),
        },
        // The arc of moods, within the range.
        Cycle::Day => Steer {
            arc: Some(state.arc.clone()),
            bpm: Some(BpmPlan {
                stages: vec![(cycles.day.0 + cycles.day.1) / 2.0],
                stage_tracks: 1,
                tolerance: (cycles.day.1 - cycles.day.0) / 2.0,
                replaces_arc: false,
            }),
        },
        Cycle::Evening => Steer {
            arc: None,
            bpm: Some(cycles.evening.plan()),
        },
    }
}

/// The moods a cycle allows whatever the listener picked: the evening is quiet music only.
pub(super) fn forced_moods(state: &State) -> Option<&'static [&'static str]> {
    (state.cycles.active == Cycle::Evening).then_some(&["calm", "sad"])
}

impl Radio {
    pub fn cycle_info(&self) -> Value {
        let state = self.st();
        let steer = steer(&state);
        let (target, stage) = match &steer.bpm {
            Some(plan) => (
                Some(plan.target_at(state.position)),
                Some(plan.stage_at(state.position)),
            ),
            None => (None, None),
        };
        let cycles = &state.cycles;
        json!({
            "active": cycles.active.name(),
            "morning": cycles.morning.json(),
            "day": {"bpm_min": cycles.day.0, "bpm_max": cycles.day.1},
            "evening": cycles.evening.json(),
            "target_bpm": target.map(|t| t.round()),
            "stage": stage,
            "mood": steer.mood_at(state.position),
            "min_bpm": MIN_BPM,
            "max_bpm": MAX_BPM,
            "min_stages": MIN_STAGES,
            "max_stages": MAX_STAGES,
            "max_stage_tracks": MAX_STAGE_TRACKS,
            "min_tolerance": MIN_TOLERANCE,
            "max_tolerance": MAX_TOLERANCE,
        })
    }

    fn cycles_json(cycles: &Cycles) -> Value {
        json!({
            "active": cycles.active.name(),
            "morning": cycles.morning.json(),
            "day": {"bpm_min": cycles.day.0, "bpm_max": cycles.day.1},
            "evening": cycles.evening.json(),
        })
    }

    /// Take up the plans kept by the last run.
    pub(super) fn restore_cycle(&self) {
        let Some(kept) = self.setting(SETTING_CYCLE) else {
            return;
        };
        if let Ok(cycles) = Self::parse_cycles(&Cycles::default(), &kept) {
            self.st().cycles = cycles;
        }
    }

    /// `current` changed as `change` says (any of "active", "morning", "day", "evening"), or why not.
    fn parse_cycles(current: &Cycles, change: &Value) -> Result<Cycles, String> {
        let mut next = current.clone();
        if let Some(name) = change.get("active") {
            next.active = name
                .as_str()
                .and_then(Cycle::of)
                .ok_or("\"active\" is off, morning, day or evening")?;
        }
        if let Some(morning) = change.get("morning") {
            next.morning = Ramp::parse(morning).map_err(|e| format!("morning: {e}"))?;
        }
        if let Some(evening) = change.get("evening") {
            next.evening = Ramp::parse(evening).map_err(|e| format!("evening: {e}"))?;
        }
        if let Some(day) = change.get("day") {
            let bound = |key: &str| {
                day.get(key)
                    .and_then(Value::as_f64)
                    .filter(|b| (MIN_BPM..=MAX_BPM).contains(b))
            };
            let (Some(low), Some(high)) = (bound("bpm_min"), bound("bpm_max")) else {
                return Err(format!(
                    "day: \"bpm_min\" and \"bpm_max\" are BPM of {MIN_BPM}..{MAX_BPM}"
                ));
            };
            if high - low < MIN_DAY_SPAN {
                return Err(format!("day: the range is at least {MIN_DAY_SPAN} BPM"));
            }
            next.day = (low, high);
        }
        Ok(next)
    }

    /// Change the plans and/or the cycle that is on. The queue follows it once it is rebuilt.
    pub async fn set_cycle(&self, change: &Value) -> Result<(), RadioError> {
        let next = Self::parse_cycles(&self.st().cycles, change).map_err(RadioError::Invalid)?;
        if next.active != Cycle::Off && self.st().saved_settings.is_none() {
            // The station is steered along the plan: its settings are put back at the end.
            self.start_mood_arc().await?;
        }
        self.st().cycles = next.clone();
        self.remember(SETTING_CYCLE, &Self::cycles_json(&next));
        if next.active == Cycle::Off {
            let _ = self.release_settings().await;
        }
        Ok(())
    }
}
