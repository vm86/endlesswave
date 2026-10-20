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
use std::time::Duration;

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

/// When each cycle begins in the day, as minutes after midnight, local time. A cycle goes on until the next one begins, the
/// evening until the morning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    pub enabled: bool,
    pub morning: u32,
    pub day: u32,
    pub evening: u32,
}

impl Schedule {
    /// The cycle that is on at this time of the day: the one whose time came last (the evening's of yesterday before the
    /// morning).
    pub fn cycle_at(&self, minutes: u32) -> Cycle {
        let starts = [
            (self.morning, Cycle::Morning),
            (self.day, Cycle::Day),
            (self.evening, Cycle::Evening),
        ];
        starts
            .iter()
            .filter(|(start, _)| *start <= minutes)
            .max_by_key(|(start, _)| *start)
            .or_else(|| starts.iter().max_by_key(|(start, _)| *start))
            .map_or(Cycle::Day, |(_, cycle)| *cycle)
    }

    fn json(&self) -> Value {
        json!({
            "enabled": self.enabled,
            "morning": clock_text(self.morning),
            "day": clock_text(self.day),
            "evening": clock_text(self.evening),
        })
    }
}

fn clock_text(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// "HH:MM" as minutes after midnight.
fn parse_clock(text: &str) -> Option<u32> {
    let (hours, minutes) = text.split_once(':')?;
    let (hours, minutes): (u32, u32) = (hours.parse().ok()?, minutes.parse().ok()?);
    (hours < 24 && minutes < 60 && text.len() == 5).then_some(hours * 60 + minutes)
}

/// The local time of day now, in minutes after midnight.
fn local_minutes(wall: f64) -> u32 {
    use chrono::{Local, TimeZone, Timelike};
    Local
        .timestamp_opt(wall as i64, 0)
        .single()
        .map_or(0, |t| t.hour() * 60 + t.minute())
}

/// The plans of the three cycles, and which is on.
#[derive(Debug, Clone, PartialEq)]
pub struct Cycles {
    pub active: Cycle,
    pub morning: Ramp,
    /// The range of the day, in BPM.
    pub day: (f64, f64),
    pub evening: Ramp,
    pub schedule: Schedule,
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
            schedule: Schedule {
                enabled: false,
                morning: 6 * 60,
                day: 11 * 60,
                evening: 18 * 60,
            },
        }
    }
}

/// What steers the choice now: the cycle's plan of the tempo and, for the day, the arc of moods.
pub(super) fn steer(state: &State) -> Steer {
    let cycles = &state.cycles;
    let offset = if cycles.active == Cycle::Off {
        0
    } else {
        state.cycle_start
    };
    match cycles.active {
        Cycle::Off => Steer {
            arc: state.arc_on.then(|| state.arc.clone()),
            bpm: None,
            offset,
        },
        Cycle::Morning => Steer {
            arc: None,
            bpm: Some(cycles.morning.plan()),
            offset,
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
            offset,
        },
        Cycle::Evening => Steer {
            arc: None,
            bpm: Some(cycles.evening.plan()),
            offset,
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
            Some(plan) => {
                let at = steer.place(state.position);
                (Some(plan.target_at(at)), Some(plan.stage_at(at)))
            }
            None => (None, None),
        };
        let cycles = &state.cycles;
        json!({
            "active": cycles.active.name(),
            "morning": cycles.morning.json(),
            "day": {"bpm_min": cycles.day.0, "bpm_max": cycles.day.1},
            "evening": cycles.evening.json(),
            "schedule": cycles.schedule.json(),
            "scheduled": cycles.schedule.cycle_at(local_minutes(self.clock.wall())).name(),
            "locks_mood": matches!(cycles.active, Cycle::Morning | Cycle::Evening),
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
            "schedule": cycles.schedule.json(),
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
        if let Some(schedule) = change.get("schedule") {
            let current = &next.schedule;
            let time = |key: &str, now: u32| -> Result<u32, String> {
                match schedule.get(key) {
                    None => Ok(now),
                    Some(Value::String(text)) => parse_clock(text)
                        .ok_or_else(|| format!("schedule: \"{key}\" is a time like 06:30")),
                    Some(_) => Err(format!("schedule: \"{key}\" is a time like 06:30")),
                }
            };
            let enabled = match schedule.get("enabled") {
                None => current.enabled,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Err("schedule: \"enabled\" is true or false".to_owned()),
            };
            let next_schedule = Schedule {
                enabled,
                morning: time("morning", current.morning)?,
                day: time("day", current.day)?,
                evening: time("evening", current.evening)?,
            };
            let times = [
                next_schedule.morning,
                next_schedule.day,
                next_schedule.evening,
            ];
            if times[0] == times[1] || times[1] == times[2] || times[0] == times[2] {
                return Err("schedule: the cycles begin at different times".to_owned());
            }
            next.schedule = next_schedule;
        }
        Ok(next)
    }

    /// Change the plans and/or the cycle that is on. The queue follows it once it is rebuilt. With the schedule on, the cycle is
    /// the clock's and is not chosen by hand.
    pub async fn set_cycle(&self, change: &Value) -> Result<(), RadioError> {
        self.put_cycle(change, true).await
    }

    /// The cycle that the moods are not the listener's to choose in (the morning and the evening have their own), if one is on.
    pub(super) fn mood_locked_by(&self) -> Option<Cycle> {
        let active = self.st().cycles.active;
        matches!(active, Cycle::Morning | Cycle::Evening).then_some(active)
    }

    /// `Err` when a cycle sets the moods, so they cannot be changed by hand.
    pub(super) fn check_mood_free(&self) -> Result<(), RadioError> {
        match self.mood_locked_by() {
            Some(cycle) => Err(RadioError::QueueChangeRefused(format!(
                "the {} cycle sets the mood",
                cycle.name()
            ))),
            None => Ok(()),
        }
    }

    async fn put_cycle(&self, change: &Value, by_hand: bool) -> Result<(), RadioError> {
        let mut next =
            Self::parse_cycles(&self.st().cycles, change).map_err(RadioError::Invalid)?;
        if next.schedule.enabled {
            if by_hand && change.get("active").is_some() {
                return Err(RadioError::QueueChangeRefused(
                    "the schedule is on: the cycle follows the clock".to_owned(),
                ));
            }
            // The clock says which cycle is on, at once when the schedule is switched on.
            next.active = next.schedule.cycle_at(local_minutes(self.clock.wall()));
        }
        if next.active != Cycle::Off && self.st().saved_settings.is_none() {
            // The station is steered along the plan: its settings are put back at the end.
            self.start_mood_arc().await?;
        }
        {
            let mut state = self.st();
            if next.active != state.cycles.active {
                // A cycle begins at its first stage.
                state.cycle_start = state.position;
            }
            state.cycles = next.clone();
        }
        self.remember(SETTING_CYCLE, &Self::cycles_json(&next));
        if next.active == Cycle::Off {
            // Putting the station's settings back goes to the service: it must not hold up the answer.
            let _ =
                tokio::time::timeout(std::time::Duration::from_secs(3), self.release_settings())
                    .await;
        }
        Ok(())
    }

    /// With the schedule on, the cycle that is on is the one that is due by the clock: put it on when it is not.
    pub async fn apply_schedule(&self) {
        let (schedule, active) = {
            let state = self.st();
            (state.cycles.schedule.clone(), state.cycles.active)
        };
        if !schedule.enabled {
            return;
        }
        let due = schedule.cycle_at(local_minutes(self.clock.wall()));
        if active == due {
            return;
        }
        tracing::info!("Schedule: {} begins", due.name());
        if let Err(error) = self.put_cycle(&json!({"active": due.name()}), false).await {
            tracing::warn!("The schedule could not put on the {}: {error}", due.name());
            return;
        }
        self.refresh_queue();
    }

    /// Background task: look at the schedule every so often, for ever.
    pub async fn watch_schedule(&self) {
        loop {
            self.apply_schedule().await;
            tokio::time::sleep(Duration::from_secs(20)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schedule() -> Schedule {
        Schedule {
            enabled: true,
            morning: 6 * 60,
            day: 11 * 60,
            evening: 18 * 60 + 30,
        }
    }

    #[test]
    fn the_cycle_of_the_time_of_day_is_the_one_whose_time_came_last() {
        let s = schedule();
        assert_eq!(
            s.cycle_at(6 * 60),
            Cycle::Morning,
            "at the minute it begins"
        );
        assert_eq!(s.cycle_at(10 * 60 + 59), Cycle::Morning);
        assert_eq!(s.cycle_at(11 * 60), Cycle::Day);
        assert_eq!(s.cycle_at(18 * 60 + 29), Cycle::Day);
        assert_eq!(s.cycle_at(18 * 60 + 30), Cycle::Evening);
        assert_eq!(s.cycle_at(23 * 60 + 59), Cycle::Evening);
        assert_eq!(
            s.cycle_at(0),
            Cycle::Evening,
            "the evening goes on past midnight"
        );
        assert_eq!(s.cycle_at(5 * 60 + 59), Cycle::Evening, "until the morning");
    }

    #[test]
    fn the_times_are_read_as_hours_and_minutes() {
        assert_eq!(parse_clock("06:30"), Some(390));
        assert_eq!(parse_clock("00:00"), Some(0));
        assert_eq!(parse_clock("23:59"), Some(23 * 60 + 59));
        for bad in ["24:00", "6:30", "06:60", "0630", "ab:cd", "", "06:30:00"] {
            assert_eq!(parse_clock(bad), None, "{bad}");
        }
        assert_eq!(clock_text(390), "06:30");
    }

    #[test]
    fn a_schedule_with_the_same_time_twice_is_refused() {
        let current = Cycles::default();
        let same = json!({"schedule": {"morning": "07:00", "day": "07:00"}});
        assert!(Radio::parse_cycles(&current, &same).is_err());
        let wrong = json!({"schedule": {"evening": "late"}});
        assert!(Radio::parse_cycles(&current, &wrong).is_err());
        let fine = json!({"schedule": {"enabled": true, "morning": "05:30"}});
        let next = Radio::parse_cycles(&current, &fine).unwrap();
        assert_eq!((next.schedule.enabled, next.schedule.morning), (true, 330));
        assert_eq!(next.schedule.day, current.schedule.day, "the rest stays");
    }
}
