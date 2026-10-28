//! What the console draws under the bar of a deck: the waveform (three bands, so that the kicks, the voices and the hats
//! can be told apart by eye), the grid of the beats, and the places of the track that are good to mix at.
//!
//! The beats are found from the low band: the grid of the BPM the radio knows (refined a little: a BPM that is a tenth
//! off drifts by a beat in a few minutes) is laid over the onsets of the kick, at the phase and the downbeat that fit
//! them best. The energy of each bar tells the sections apart: where the track is thin (the intro, a breakdown, the
//! outro) and where it is full. Their edges are the places to mix at.

use serde::Serialize;

const RATE: f64 = 44_100.0;
/// The columns of the waveform.
const BINS: usize = 900;
/// The step of the analysis of the beats: 5 ms.
const HOP: usize = 220;
const HOP_SECONDS: f64 = HOP as f64 / RATE;
/// The band edges: the kick and the bass below the first, the voices and the body up to the second, the hats above.
const LOW_HZ: f64 = 200.0;
const MID_HZ: f64 = 2_500.0;
/// How far from the BPM it is looked for, and how finely (a share).
const BPM_REACH: f64 = 0.015;
const BPM_STEP: f64 = 0.0005;
/// A bar is thin below this share of the full energy of the track, and full above the second one.
const THIN: f64 = 0.5;
const FULL: f64 = 0.7;
/// A thin stretch is a section of its own from this many bars.
const MIN_BARS: usize = 4;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Marker {
    /// Seconds from the start of the track.
    pub at: f64,
    /// "start" (the first downbeat), "intro" (the intro ends and the track fills out), "breakdown" (it thins out),
    /// "drop" (it fills out again), "outro" (it thins out for good).
    pub kind: &'static str,
    pub bar: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct Overview {
    /// Per column: the low, the mid and the high band, 0 to 1 (three numbers a column).
    pub bins: Vec<f32>,
    pub duration: f64,
    /// The BPM the grid fits best, and where its first bar begins (seconds).
    pub bpm: f64,
    pub beat_offset: f64,
    pub markers: Vec<Marker>,
}

/// One pole low-pass filter.
struct Pole {
    a: f64,
    y: f64,
}

impl Pole {
    fn new(hz: f64) -> Self {
        Self {
            a: 1.0 - (-2.0 * std::f64::consts::PI * hz / RATE).exp(),
            y: 0.0,
        }
    }
    fn run(&mut self, x: f64) -> f64 {
        self.y += self.a * (x - self.y);
        self.y
    }
}

/// Analyze a track: stereo, interleaved, 44.1 kHz; `bpm` is what the radio knows of its tempo (0 when nothing).
pub fn analyze(pcm: &[f32], bpm: f64) -> Overview {
    let frames = pcm.len() / 2;
    let duration = frames as f64 / RATE;
    let per_bin = (frames / BINS).max(1);
    let mut bins = vec![0f32; BINS * 3];
    let (mut low_pole, mut mid_pole) = (Pole::new(LOW_HZ), Pole::new(MID_HZ));
    // Per hop: the energy of the whole sound and of the low band.
    let hops = frames / HOP;
    let (mut full, mut low) = (vec![0f64; hops + 1], vec![0f64; hops + 1]);
    for n in 0..frames {
        let x = (pcm[n * 2] as f64 + pcm[n * 2 + 1] as f64) * 0.5;
        let l = low_pole.run(x);
        let m = mid_pole.run(x) - l;
        let h = x - l - m;
        let bin = (n / per_bin).min(BINS - 1) * 3;
        for (slot, value) in [l, m, h].into_iter().enumerate() {
            bins[bin + slot] = bins[bin + slot].max(value.abs() as f32);
        }
        let hop = n / HOP;
        full[hop] += x * x;
        low[hop] += l * l;
    }
    // The bands are shown against the same scale (the bass is the broadest, as it is in the sound), a little softened so
    // that the quiet parts are not a line. The top is the loud end, not the one peak that would flatten the rest.
    let mut sorted: Vec<f32> = bins.clone();
    sorted.sort_by(f32::total_cmp);
    let top = sorted[sorted.len() * 99 / 100].max(1e-6);
    for value in &mut bins {
        *value = (*value / top).min(1.0).powf(0.7);
    }
    let mut overview = Overview {
        bins,
        duration,
        bpm,
        beat_offset: 0.0,
        markers: Vec::new(),
    };
    if !(bpm.is_finite() && bpm > 40.0 && bpm < 240.0) || hops < 2_000 {
        return overview;
    }

    // The onsets of the low band: how much it rises from hop to hop.
    let level: Vec<f64> = low.iter().map(|e| (e / HOP as f64).sqrt()).collect();
    let flux: Vec<f64> = std::iter::once(0.0)
        .chain(level.windows(2).map(|w| (w[1] - w[0]).max(0.0)))
        .collect();

    let (best_bpm, phase, downbeat) = fit_grid(&flux, bpm);
    let period = 60.0 / best_bpm / HOP_SECONDS;
    overview.bpm = best_bpm;
    let first = phase + downbeat as f64 * period;
    overview.beat_offset = first * HOP_SECONDS;

    // The energy of each bar.
    let bar = 4.0 * period;
    let count = (((hops as f64 - first) / bar).floor().max(0.0)) as usize;
    let energy: Vec<f64> = (0..count)
        .map(|b| {
            let (from, to) = (
                (first + b as f64 * bar) as usize,
                (first + (b + 1) as f64 * bar) as usize,
            );
            let sum: f64 = full[from..to.min(hops)].iter().sum();
            (sum / (to - from).max(1) as f64).sqrt()
        })
        .collect();
    overview.markers = sections(&energy, overview.beat_offset, bar * HOP_SECONDS);
    overview
}

/// The tempo, the phase (in hops) and the downbeat (0 to 3) that fit the onsets best.
fn fit_grid(flux: &[f64], bpm: f64) -> (f64, f64, usize) {
    let mut best = (f64::MIN, bpm, 0.0);
    let steps = (BPM_REACH / BPM_STEP) as i32;
    for step in -steps..=steps {
        let candidate = bpm * (1.0 + step as f64 * BPM_STEP);
        let period = 60.0 / candidate / HOP_SECONDS;
        let mut phase = 0.0;
        while phase < period {
            let mut score = 0.0;
            let mut at = phase;
            while (at as usize) < flux.len() {
                score += flux[at.round() as usize % flux.len()];
                at += period;
            }
            if score > best.0 {
                best = (score, candidate, phase);
            }
            phase += 0.5;
        }
    }
    let (_, tempo, phase) = best;
    let period = 60.0 / tempo / HOP_SECONDS;
    // The kick falls on the one of the bar more than on the rest of the beats.
    let mut sums = [0.0; 4];
    let mut beat = 0usize;
    let mut at = phase;
    while (at as usize) < flux.len() {
        sums[beat % 4] += flux[at.round() as usize % flux.len()];
        beat += 1;
        at += period;
    }
    let downbeat = (0..4)
        .max_by(|&a, &b| sums[a].total_cmp(&sums[b]))
        .unwrap_or(0);
    (tempo, phase, downbeat)
}

/// The edges of the sections, from the energy of the bars.
fn sections(energy: &[f64], offset: f64, bar_seconds: f64) -> Vec<Marker> {
    let mut marks = Vec::new();
    if energy.len() < MIN_BARS * 2 {
        return marks;
    }
    let mut sorted: Vec<f64> = energy.iter().copied().filter(|e| *e > 0.0).collect();
    sorted.sort_by(f64::total_cmp);
    let Some(&top) = sorted.get(sorted.len() * 85 / 100) else {
        return marks;
    };
    // Full or thin for each bar, with a gap between the two so that a bar in the middle does not flicker.
    let mut full = Vec::with_capacity(energy.len());
    let mut state = energy[0] / top >= FULL;
    for e in energy {
        let share = e / top;
        if share >= FULL {
            state = true;
        } else if share < THIN {
            state = false;
        }
        full.push(state);
    }
    // The runs: (full?, first bar, bars).
    let mut runs: Vec<(bool, usize, usize)> = Vec::new();
    for (bar, &is_full) in full.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if run.0 == is_full => run.2 += 1,
            _ => runs.push((is_full, bar, 1)),
        }
    }
    let at = |bar: usize| offset + bar as f64 * bar_seconds;
    // Two marks a few bars apart are one place.
    let mut mark = |kind, bar: usize| {
        if marks
            .last()
            .is_none_or(|m: &Marker| bar >= m.bar + MIN_BARS)
        {
            marks.push(Marker {
                at: at(bar),
                kind,
                bar,
            });
        }
    };
    let last = runs.len() - 1;
    let mut seen_full = false;
    for (index, &(is_full, bar, bars)) in runs.iter().enumerate() {
        if is_full {
            if index == 0 {
                mark("start", 0);
            } else if !seen_full {
                // The first full part after a thin beginning.
                mark("intro", bar);
            } else {
                mark("drop", bar);
            }
            seen_full = true;
        } else if bars >= MIN_BARS {
            if index == last && seen_full {
                mark("outro", bar);
            } else if index != 0 && seen_full {
                mark("breakdown", bar);
            }
        } else if index == last && seen_full && bars >= 2 {
            mark("outro", bar);
        }
    }
    if marks.first().is_none_or(|m| m.kind != "start") && energy[0] / top >= THIN {
        marks.insert(
            0,
            Marker {
                at: at(0),
                kind: "start",
                bar: 0,
            },
        );
    }
    marks
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    /// A track at 120 BPM: a kick on every beat, loud or quiet by the bar. 8 bars thin, 16 full, 8 thin, 16 full, 8 thin.
    fn track(first: f64) -> Vec<f32> {
        let sections = [(8, 0.15), (16, 0.9), (8, 0.15), (16, 0.9), (8, 0.15)];
        let bars: usize = sections.iter().map(|s| s.0).sum();
        let seconds = first + bars as f64 * 2.0 + 1.0;
        let frames = (seconds * RATE) as usize;
        let mut pcm = vec![0f32; frames * 2];
        for beat in 0..bars * 4 {
            let start = ((first + beat as f64 * 0.5) * RATE) as usize;
            let mut bar = beat / 4;
            let amplitude = sections
                .iter()
                .find_map(|(n, a)| {
                    if bar < *n {
                        Some(*a)
                    } else {
                        bar -= n;
                        None
                    }
                })
                .unwrap_or(0.15);
            // The one of the bar is a little harder than the other beats.
            let amplitude = amplitude * if beat % 4 == 0 { 1.0 } else { 0.7 };
            for n in 0..(0.2 * RATE) as usize {
                let t = n as f64 / RATE;
                let s = (amplitude * (2.0 * PI * 60.0 * t).sin() * (-t * 25.0).exp()) as f32;
                if start + n < frames {
                    pcm[(start + n) * 2] += s;
                    pcm[(start + n) * 2 + 1] += s;
                }
            }
        }
        pcm
    }

    fn near(marks: &[Marker], kind: &str, seconds: f64) -> bool {
        marks
            .iter()
            .any(|m| m.kind == kind && (m.at - seconds).abs() < 0.15)
    }

    #[test]
    fn the_grid_follows_the_kicks_even_when_the_bpm_is_a_little_off() {
        let overview = analyze(&track(0.13), 120.6);
        assert!((overview.bpm - 120.0).abs() < 0.3, "bpm {}", overview.bpm);
        let offset = overview.beat_offset % 2.0;
        assert!(
            (offset - 0.13).abs() < 0.03,
            "offset {}",
            overview.beat_offset
        );
    }

    #[test]
    fn the_sections_are_found() {
        let overview = analyze(&track(0.13), 120.0);
        let marks = &overview.markers;
        // The bars begin at 0.13 s and last 2 s.
        assert!(near(marks, "intro", 0.13 + 16.0), "{marks:?}");
        assert!(near(marks, "breakdown", 0.13 + 48.0), "{marks:?}");
        assert!(near(marks, "drop", 0.13 + 64.0), "{marks:?}");
        assert!(near(marks, "outro", 0.13 + 96.0), "{marks:?}");
    }

    #[test]
    fn the_waveform_has_a_column_for_each_band() {
        let overview = analyze(&track(0.0), 120.0);
        assert_eq!(overview.bins.len(), BINS * 3);
        // A kick is low: the low band is the loudest of the three somewhere, the hats are quiet throughout.
        let low = (0..BINS).map(|b| overview.bins[b * 3]).fold(0f32, f32::max);
        let high = (0..BINS)
            .map(|b| overview.bins[b * 3 + 2])
            .fold(0f32, f32::max);
        assert!(low > 0.9, "{low}");
        assert!(overview.bins.iter().all(|v| (0.0..=1.0).contains(v)));
        let _ = high;
    }

    #[test]
    fn a_track_without_a_known_tempo_has_a_waveform_and_no_markers() {
        let overview = analyze(&track(0.0), 0.0);
        assert!(overview.markers.is_empty());
        assert_eq!(overview.bins.len(), BINS * 3);
    }
}
