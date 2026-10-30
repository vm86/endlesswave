//! What a pair of tracks needs for a blend, as far as choosing the next track goes (the part of task 1.6 of
//! docs/rust-port.md that `transition_cost` uses): the speed, the section, the kind of blend.
//! Ported from `dj_mix.py`: `tempo_ratio`, `transition_speed`, `section_length`, `choose_transition`.

use super::{
    SAMPLE_RATE, TrackAnalysis, key,
    phrase::{Kind, Phrase},
};

/// Largest tempo change a blend glides through to match the previous track.
const MAX_TEMPO_SHIFT: f64 = 0.15;
/// Below this the tempos are taken for equal and nothing is stretched.
const NEGLIGIBLE_TEMPO_SHIFT: f64 = 0.002;
/// Intros and outros shorter than this don't count as a section.
const MIN_SECTION_SECONDS: f64 = 4.0;
/// Tracks that overlap this long get in each other's way with their mids.
const EQ_BLEND_MIN_SECONDS: f64 = 10.0;
/// Loudness gap (dB) from which a beat-matched pair is swept, not swapped.
const SWEEP_LOUDNESS_GAP_DB: f64 = 3.0;

/// Playback speed bringing `bpm` to `target_bpm` (half or double time allowed).
pub fn tempo_ratio(target_bpm: f64, bpm: f64) -> f64 {
    [0.5, 1.0, 2.0]
        .map(|factor| target_bpm / (bpm * factor))
        .into_iter()
        .min_by(|a, b| a.log2().abs().total_cmp(&b.log2().abs()))
        .expect("there are three")
}

/// Speed to play the following track at, 0 when it can't be matched.
pub fn transition_speed(current_bpm: f64, following: &TrackAnalysis) -> f64 {
    if current_bpm == 0.0 || following.bpm == 0.0 {
        return 0.0;
    }
    let speed = tempo_ratio(current_bpm, following.bpm);
    if (speed - 1.0).abs() > MAX_TEMPO_SHIFT {
        return 0.0;
    }
    if (speed - 1.0).abs() < NEGLIGIBLE_TEMPO_SHIFT {
        1.0
    } else {
        speed
    }
}

/// The shorter of the outgoing outro and the incoming intro (samples): how Mixxx's AutoDJ sizes a transition.
/// `None` when one of the sections is unknown or too short to speak of.
pub fn section_length(current: &TrackAnalysis, following: &TrackAnalysis) -> Option<i64> {
    if current.outro_start < 0 || following.intro_end < 0 {
        return None;
    }
    let outro = current.end - current.outro_start;
    let intro = following.intro_end - following.start;
    let shortest = outro.min(intro);
    (shortest as f64 >= MIN_SECTION_SECONDS * f64::from(SAMPLE_RATE)).then_some(shortest)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transition {
    Crossfade,
    BassSwap,
    EqBlend,
    FilterSweep,
}

impl Transition {
    /// The name the API and the statistics give it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Crossfade => "crossfade",
            Self::BassSwap => "bass swap",
            Self::EqBlend => "eq blend",
            Self::FilterSweep => "filter sweep",
        }
    }
}

/// Pick the kind of blend that suits the pair of tracks. Two tracks on the same beat grid share the highs
/// while the bass is handed over; when they overlap for long, or their keys clash, the mids are swapped too;
/// without a shared grid (or with a big jump in loudness) one track is filtered away while the other is
/// filtered in.
pub fn choose_transition(
    current: &TrackAnalysis,
    following: &TrackAnalysis,
    beat_matched: bool,
    blend_seconds: f64,
) -> Transition {
    let loudness_gap = (current.loudness_db - following.loudness_db).abs();
    if !beat_matched || loudness_gap >= SWEEP_LOUDNESS_GAP_DB {
        return Transition::FilterSweep;
    }
    let relation = key::key_relation(current.key, following.key);
    if blend_seconds >= EQ_BLEND_MIN_SECONDS || key::key_quality(relation) == key::KeyQuality::Poor
    {
        Transition::EqBlend
    } else {
        Transition::BassSwap
    }
}

// Where a blend goes, task 1.6 of docs/rust-port.md. Ported from `dj_mix.py`: `_blend_beats`, `_first_beat`,
// `plan_matched`, `plan_early`, `plan_blend`.

/// Blend length when the structure of the tracks is unknown.
const BEATS_PER_BLEND: i64 = 16;
const PLAIN_BLEND_SECONDS: f64 = 8.0;
/// Blends follow the shorter of the outgoing outro and the incoming intro, within these limits (beats are
/// counted in steps of two bars).
const MIN_BLEND_BEATS: i64 = 8;
const MAX_BLEND_BEATS: i64 = 32;
const BLEND_BEATS_STEP: i64 = 8;
const MIN_PLAIN_BLEND_SECONDS: f64 = 4.0;
const MAX_PLAIN_BLEND_SECONDS: f64 = 16.0;
/// A matched blend must leave at least this much of the outgoing track.
const MIN_SOLO_SECONDS: f64 = 10.0;
/// A blend asked for at once (the skip button) starts no sooner than this after the request, on the first bar
/// line of the outgoing track after it.
const EARLY_LEAD_SECONDS: f64 = 2.0;
/// Less room than this in either track and the request is let go.
pub const MIN_EARLY_BLEND_SECONDS: f64 = 4.0;
/// A bar is this many beats.
const BEATS_PER_BAR: f64 = 4.0;

/// Python's `round()` of a float: halves go to the even number.
fn round(value: f64) -> i64 {
    value.round_ties_even() as i64
}

/// Where the blend starts in each track and how long it lasts, in samples of the outgoing track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub start: i64,
    pub following_start: i64,
    pub length: i64,
}

impl TrackAnalysis {
    /// Samples per beat. The tempo must be known.
    pub fn beat_period(&self) -> f64 {
        f64::from(SAMPLE_RATE) * 60.0 / self.bpm
    }
}

fn blend_beats(section: Option<i64>, period: f64) -> i64 {
    let Some(section) = section else {
        return BEATS_PER_BLEND;
    };
    let beats = round(section as f64 / period / BLEND_BEATS_STEP as f64) * BLEND_BEATS_STEP;
    beats.clamp(MIN_BLEND_BEATS, MAX_BLEND_BEATS)
}

/// The first beat of the track after its silence (samples).
fn first_beat(track: &TrackAnalysis) -> i64 {
    let j = ((track.start as f64 - track.head_beat) / track.beat_period()).ceil();
    round(track.head_beat + j * track.beat_period()).max(0)
}

/// Plan a blend on the beat, `None` if there is no room for one. `following` is at its own tempo: the blend
/// glides to it, so it takes `speed` times as much of the following track as of the outgoing one.
pub fn plan_matched(
    current: &TrackAnalysis,
    following: &TrackAnalysis,
    speed: f64,
) -> Option<Plan> {
    let period = current.beat_period();
    let solo = round(MIN_SOLO_SECONDS * f64::from(SAMPLE_RATE));
    let following_start = first_beat(following);

    let section = section_length(current, following);
    let mut beats = blend_beats(section, period);
    let current_start = loop {
        let length = round(beats as f64 * period);
        // The blend ends with the outgoing track, on a beat of its grid.
        let k = ((current.end - length) as f64 - current.tail_beat) / period;
        let current_start = round(current.tail_beat + k.floor() * period);
        if section.is_none() || beats <= MIN_BLEND_BEATS {
            break current_start;
        }
        // Rounding up to whole bars mustn't reach into the cores.
        let into_outgoing_core = (current_start as f64) < current.outro_start as f64 - period / 2.0;
        let into_incoming_core = following_start as f64 + length as f64 * speed
            > following.intro_end as f64 + period / 2.0;
        if !(into_outgoing_core || into_incoming_core) {
            break current_start;
        }
        beats -= BLEND_BEATS_STEP;
    };

    if current_start < current.start + solo {
        return None;
    }
    let length = (current.end - current_start)
        .min(((following.end - following_start) as f64 / speed) as i64);
    Some(Plan {
        start: current_start,
        following_start,
        length,
    })
}

/// Align complete 8/16/32-bar phrases in the final quarter of the outgoing track.
/// Prefer thinning sections and resolve the incoming intro/breakdown at a build or drop.
/// With insufficient room the caller falls back to its original beat-matched plan.
pub fn plan_phrase(
    current: &TrackAnalysis,
    outgoing: &Phrase,
    following: &TrackAnalysis,
    incoming: &Phrase,
    speed: f64,
    position: i64,
) -> Result<Plan, &'static str> {
    if !speed.is_finite()
        || speed <= 0.0
        || !current.bpm.is_finite()
        || current.bpm <= 0.0
        || !following.bpm.is_finite()
        || following.bpm <= 0.0
    {
        return Err("the tempo of a track is not known");
    }
    let earliest = position
        .max(current.start + round(MIN_SOLO_SECONDS * SAMPLE_RATE as f64))
        .max(current.start + (current.end - current.start) * 3 / 4);
    let out_points = outgoing.boundaries(earliest, current.end);
    let in_points = incoming.boundaries(following.start, following.end);
    let near = |a: i64, b: i64, beat: f64| (a - b).abs() as f64 <= beat * 0.1;
    let mut best: Option<(f64, Plan)> = None;
    for &start in &out_points {
        for bars in [8, 16, 32] {
            let length = round(outgoing.bar * bars as f64);
            let end = start + length;
            if end > current.end
                || !outgoing
                    .boundaries(end - 1, end + 1)
                    .iter()
                    .any(|&at| near(at, end, outgoing.beat()))
            {
                continue;
            }
            for &following_start in &in_points {
                let in_end = following_start + round(length as f64 * speed);
                if in_end > following.end {
                    continue;
                }
                // Half/double-time pairs must span whole phrases in both tracks.
                let in_bars = length as f64 * speed / incoming.bar;
                if ![8.0, 16.0, 32.0].iter().any(|n| (in_bars - n).abs() < 0.05)
                    || !in_points
                        .iter()
                        .any(|&at| near(at, in_end, incoming.beat()))
                {
                    continue;
                }
                let first_lift = incoming
                    .markers
                    .iter()
                    .find(|m| matches!(m.kind, Kind::Intro | Kind::Drop) && m.at > following.start);
                if first_lift.is_some_and(|m| following_start >= m.at) {
                    continue;
                }
                let out_section = outgoing.markers.iter().any(|m| {
                    (matches!(m.kind, Kind::Outro | Kind::Breakdown)
                        && near(m.at, start, outgoing.beat()))
                        || (matches!(m.kind, Kind::Intro | Kind::Drop)
                            && near(m.at, end, outgoing.beat()))
                });
                let in_section = incoming.markers.iter().any(|m| {
                    matches!(m.kind, Kind::Intro | Kind::Drop)
                        && near(m.at, in_end, incoming.beat())
                });
                let thin_entry = incoming.markers.iter().any(|m| {
                    matches!(m.kind, Kind::Start | Kind::Breakdown)
                        && near(m.at, following_start, incoming.beat())
                });
                let score = (if out_section { 4.0 } else { 0.0 })
                    + (if in_section { 4.0 } else { 0.0 })
                    + (if thin_entry { 1.0 } else { 0.0 })
                    + (end - current.start) as f64 / (current.end - current.start).max(1) as f64
                    - (following_start - following.start) as f64 / SAMPLE_RATE as f64 * 0.001;
                if best.as_ref().is_none_or(|(previous, _)| score > *previous) {
                    best = Some((
                        score,
                        Plan {
                            start,
                            following_start,
                            length,
                        },
                    ));
                }
            }
        }
    }
    best.map(|(_, plan)| plan)
        .ok_or("no complete compatible phrases left")
}

/// Plan a blend that is asked for while the outgoing track is at `position`. It starts on a bar line of the
/// outgoing track, the first one at least `EARLY_LEAD_SECONDS` ahead (the beat grid is taken from the end of the
/// track that is nearer), lasts as long as any blend without sections and comes in on the first beat of the
/// following track. Without a shared grid it starts as soon as the lead allows. `None` if the outgoing track
/// has no room left for it.
pub fn plan_early(
    current: &TrackAnalysis,
    following: &TrackAnalysis,
    position: i64,
    beat_matched: bool,
    speed: f64,
) -> Option<Plan> {
    let earliest = position + round(EARLY_LEAD_SECONDS * f64::from(SAMPLE_RATE));
    let (start, following_start, length);
    if beat_matched && current.bpm != 0.0 && following.bpm != 0.0 {
        let period = current.beat_period();
        let anchor = if (position as f64 - current.head_beat) < current.tail_beat - position as f64
        {
            current.head_beat
        } else {
            current.tail_beat
        };
        let bar = BEATS_PER_BAR * period;
        start = round(anchor + ((earliest as f64 - anchor) / bar).ceil() * bar);
        following_start = first_beat(following);
        length = round(BEATS_PER_BLEND as f64 * period);
    } else {
        start = earliest;
        following_start = following.start;
        length = round(PLAIN_BLEND_SECONDS * f64::from(SAMPLE_RATE));
    }
    let length = length
        .min(current.end - start)
        .min(((following.end - following_start) as f64 / speed) as i64);
    if (length as f64) < MIN_EARLY_BLEND_SECONDS * f64::from(SAMPLE_RATE) {
        return None;
    }
    Some(Plan {
        start,
        following_start,
        length,
    })
}

/// Where the blend starts in each track and how long it lasts.
pub fn plan_blend(
    current: &TrackAnalysis,
    following: &TrackAnalysis,
    beat_matched: bool,
    speed: f64,
) -> Plan {
    if beat_matched && current.bpm != 0.0 && following.bpm != 0.0 {
        if let Some(planned) = plan_matched(current, following, speed) {
            return planned;
        }
    }

    let section = section_length(current, following);
    let seconds = section.map_or(PLAIN_BLEND_SECONDS, |section| {
        (section as f64 / f64::from(SAMPLE_RATE))
            .max(MIN_PLAIN_BLEND_SECONDS)
            .min(MAX_PLAIN_BLEND_SECONDS)
    });
    let length = round(seconds * f64::from(SAMPLE_RATE))
        .min((current.end - current.start).div_euclid(2))
        .min((following.end - following.start).div_euclid(2));
    Plan {
        start: current.end - length,
        following_start: following.start,
        length,
    }
}

#[cfg(test)]
mod phrase_tests {
    use super::*;

    const RATE: f64 = SAMPLE_RATE as f64;

    /// A track at 120 BPM (a bar is 2 s) whose first bar is at `first` samples.
    fn track(seconds: f64, first: f64) -> (TrackAnalysis, Phrase) {
        let bar = 2.0 * RATE;
        let end = (seconds * RATE) as i64;
        let last = first + ((end as f64 - first) / (bar / 4.0)).floor() * (bar / 4.0);
        let analysis = TrackAnalysis {
            bpm: 120.0,
            head_beat: first,
            tail_beat: last,
            start: 0,
            end,
            loudness_db: -9.0,
            intro_end: (16.0 * RATE) as i64,
            outro_start: ((seconds - 16.0) * RATE) as i64,
            key: 0,
        };
        let phrase = Phrase {
            bpm: 120.0,
            first_bar: first,
            bar,
            markers: Vec::new(),
        };
        (analysis, phrase)
    }

    #[test]
    fn a_blend_by_phrases_lies_bar_on_bar() {
        let (current, out) = track(180.0, 0.13 * RATE);
        let (following, into) = track(200.0, 0.4 * RATE);
        let position = (140.0 * RATE) as i64;
        let plan = plan_phrase(&current, &out, &following, &into, 1.0, position).unwrap();
        let on_bar = |phrase: &Phrase, sample: i64| {
            ((sample as f64 - phrase.first_bar) / phrase.bar)
                .fract()
                .abs()
                < 1e-3
        };
        assert!(on_bar(&out, plan.start), "{plan:?}");
        assert!(on_bar(&into, plan.following_start), "{plan:?}");
        assert!(plan.start >= position);
        assert!(plan.start + plan.length <= current.end);
        // At least 8 beats, and it ends with the outgoing track.
        assert!(plan.length as f64 >= 8.0 * out.beat());
        assert!([8.0, 16.0, 32.0].contains(&(plan.length as f64 / out.bar)));
    }

    #[test]
    fn a_blend_does_not_begin_before_the_position() {
        let (current, out) = track(180.0, 0.13 * RATE);
        let (following, into) = track(200.0, 0.13 * RATE);
        let position = (176.0 * RATE) as i64;
        // Four seconds are left: no room for a blend of 8 beats and the 10 s of the track that must stay.
        assert!(plan_phrase(&current, &out, &following, &into, 1.0, position).is_err());
    }

    fn marker(kind: Kind, seconds: f64) -> super::super::phrase::Marker {
        super::super::phrase::Marker {
            kind,
            at: (seconds * RATE) as i64,
            bar: (seconds / 2.0) as usize,
        }
    }

    #[test]
    fn outro_and_incoming_intro_resolve_as_a_complete_phrase() {
        for bars in [8, 16, 32] {
            let (current, mut out) = track(512.0, 0.0);
            let (following, mut into) = track(300.0, 0.0);
            out.markers = vec![marker(Kind::Outro, 384.0)];
            into.markers = vec![marker(Kind::Intro, bars as f64 * 2.0)];
            let plan = plan_phrase(
                &current,
                &out,
                &following,
                &into,
                1.0,
                380 * SAMPLE_RATE as i64,
            )
            .unwrap();
            assert_eq!(plan.start, 384 * SAMPLE_RATE as i64);
            assert_eq!(plan.following_start, 0);
            assert_eq!(plan.length, bars * 2 * SAMPLE_RATE as i64);
        }
    }

    #[test]
    fn a_late_breakdown_can_resolve_at_a_drop_in_both_tracks() {
        let (current, mut out) = track(180.0, 0.0);
        let (following, mut into) = track(200.0, 0.0);
        out.markers = vec![marker(Kind::Breakdown, 144.0), marker(Kind::Drop, 160.0)];
        into.markers = vec![marker(Kind::Breakdown, 0.0), marker(Kind::Drop, 16.0)];
        let plan = plan_phrase(
            &current,
            &out,
            &following,
            &into,
            1.0,
            140 * SAMPLE_RATE as i64,
        )
        .unwrap();
        assert_eq!(plan.start, 144 * SAMPLE_RATE as i64);
        assert_eq!(plan.following_start, 0);
        assert_eq!(plan.length, 16 * SAMPLE_RATE as i64);
    }

    #[test]
    fn phrase_lengths_account_for_incoming_playback_speed() {
        let (current, mut out) = track(512.0, 0.0);
        let (mut following, mut into) = track(300.0, 0.0);
        following.bpm = 125.0;
        into.bar = 60.0 / 125.0 * 4.0 * RATE;
        into.bpm = 125.0;
        out.markers = vec![marker(Kind::Outro, 384.0)];
        into.markers = vec![marker(Kind::Intro, 15.36)];
        let plan = plan_phrase(
            &current,
            &out,
            &following,
            &into,
            0.96,
            380 * SAMPLE_RATE as i64,
        )
        .unwrap();
        assert_eq!(plan.length, 16 * SAMPLE_RATE as i64);
        assert!((plan.length as f64 * 0.96 - 8.0 * into.bar).abs() < 1.0);
        assert!(plan_phrase(&current, &out, &following, &into, f64::NAN, 0).is_err());
    }

    #[test]
    fn a_track_of_unknown_tempo_gets_no_blend_by_phrases() {
        let (mut current, out) = track(180.0, 0.13 * RATE);
        let (following, into) = track(200.0, 0.13 * RATE);
        current.bpm = 0.0;
        let position = (140.0 * RATE) as i64;
        assert!(plan_phrase(&current, &out, &following, &into, 1.0, position).is_err());
    }
}
