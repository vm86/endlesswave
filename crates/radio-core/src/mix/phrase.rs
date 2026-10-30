//! The phrases of a track: the grid of its bars, and the places where it fills out or thins out. The mix lays the
//! tracks over each other by them (bar on bar, an intro under an outro), and the console of the app draws them.
//!
//! The beats are found from the low band: the grid of the BPM the analysis knows (refined a little: a BPM that is a
//! tenth off drifts by a beat in a few minutes) is laid over the onsets of the kick, at the phase and the downbeat that
//! fit them best. The energy of each bar tells the sections apart: where the track is thin (the intro, a breakdown, the
//! outro) and where it is full. Their edges are the places to mix at.

use super::SAMPLE_RATE;

const RATE: f64 = SAMPLE_RATE as f64;
/// The step of the analysis of the beats: 5 ms.
const HOP: usize = 220;
const HOP_SECONDS: f64 = HOP as f64 / RATE;
/// The band of the kick and the bass.
const LOW_HZ: f64 = 200.0;
/// How far from the BPM it is looked for, and how finely (a share).
const BPM_REACH: f64 = 0.015;
const BPM_STEP: f64 = 0.0005;
/// A bar is thin below this share of the full energy of the track, and full above the second one.
const THIN: f64 = 0.5;
const FULL: f64 = 0.7;
/// A thin stretch is a section of its own from this many bars; marks closer than this are one place.
const MIN_BARS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The first downbeat: a clean place to start the track.
    Start,
    /// The intro ends and the track fills out.
    Intro,
    /// It thins out.
    Breakdown,
    /// It fills out again.
    Drop,
    /// It thins out for good.
    Outro,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Intro => "intro",
            Self::Breakdown => "breakdown",
            Self::Drop => "drop",
            Self::Outro => "outro",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Marker {
    /// Samples from the start of the track.
    pub at: i64,
    pub kind: Kind,
    /// The bar of the grid.
    pub bar: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Phrase {
    /// The BPM the grid fits best.
    pub bpm: f64,
    /// Samples: where the first bar begins (the first downbeat found; it may lie in the silence before the music).
    pub first_bar: f64,
    /// Samples in a bar.
    pub bar: f64,
    pub markers: Vec<Marker>,
}

impl Phrase {
    /// Samples per beat.
    pub fn beat(&self) -> f64 {
        self.bar / 4.0
    }

    /// The first bar line at or after `sample` (samples).
    pub fn bar_at_or_after(&self, sample: f64) -> f64 {
        self.first_bar + ((sample - self.first_bar) / self.bar).ceil() * self.bar
    }

    /// The last bar line at or before `sample` (samples).
    pub fn bar_at_or_before(&self, sample: f64) -> f64 {
        self.first_bar + ((sample - self.first_bar) / self.bar).floor() * self.bar
    }

    /// Eight-bar phrase boundaries, restarted at each detected section edge.
    /// Sixteen and thirty-two bars are groups of these structural estimates.
    pub fn boundaries(&self, from: i64, to: i64) -> Vec<i64> {
        let mut anchors = vec![self.first_bar];
        anchors.extend(self.markers.iter().map(|m| m.at as f64));
        anchors.sort_by(f64::total_cmp);
        let mut result = Vec::new();
        for (index, &anchor) in anchors.iter().enumerate() {
            let end = anchors.get(index + 1).copied().unwrap_or(to as f64);
            let step = self.bar * 8.0;
            let mut at = anchor + ((from as f64 - anchor) / step).ceil().max(0.0) * step;
            while at <= end && at <= to as f64 {
                let sample = at.round() as i64;
                if sample >= from {
                    result.push(sample);
                }
                at += step;
            }
        }
        result.sort_unstable();
        result.dedup();
        result
    }

    /// The first mark of a kind from `from` on.
    pub fn first(&self, kind: Kind, from: i64) -> Option<&Marker> {
        self.markers.iter().find(|m| m.kind == kind && m.at >= from)
    }
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

/// The energy of each hop of the track (the whole sound), and how much the low band rises from hop to hop.
fn hop_energies(pcm: &[f32]) -> (Vec<f64>, Vec<f64>) {
    let frames = pcm.len() / 2;
    let hops = frames / HOP;
    let mut low_pole = Pole::new(LOW_HZ);
    let (mut full, mut low) = (vec![0f64; hops + 1], vec![0f64; hops + 1]);
    for n in 0..frames {
        let x = (pcm[n * 2] as f64 + pcm[n * 2 + 1] as f64) * 0.5;
        let l = low_pole.run(x);
        let hop = n / HOP;
        full[hop] += x * x;
        low[hop] += l * l;
    }
    let level: Vec<f64> = low.iter().map(|e| (e / HOP as f64).sqrt()).collect();
    let flux = std::iter::once(0.0)
        .chain(level.windows(2).map(|w| (w[1] - w[0]).max(0.0)))
        .collect();
    (full, flux)
}

/// The bars from `first` (hops) on, `period` hops to a beat: their energy, and so the sections.
fn phrase_on(full: &[f64], first: f64, period: f64, bpm: f64) -> Phrase {
    let hops = full.len() - 1;
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
    let (first_bar, bar_samples) = (first * HOP as f64, bar * HOP as f64);
    Phrase {
        bpm,
        first_bar,
        bar: bar_samples,
        markers: sections(&energy, first_bar, bar_samples),
    }
}

/// Find the phrases of a track on a grid of its own: stereo, interleaved, 44.1 kHz; `bpm` is what the analysis knows of
/// its tempo. The tempo is refined a little and the grid is laid on the kicks. `None` when the tempo is not known or the
/// track is too short to tell. This is the grid the console draws.
pub fn analyze(pcm: &[f32], bpm: f64) -> Option<Phrase> {
    let hops = pcm.len() / 2 / HOP;
    if !(bpm.is_finite() && bpm > 40.0 && bpm < 240.0) || hops < 2_000 {
        return None;
    }
    let (full, flux) = hop_energies(pcm);
    let (best_bpm, phase, downbeat) = fit_grid(&flux, bpm);
    let period = 60.0 / best_bpm / HOP_SECONDS;
    Some(phrase_on(
        &full,
        phase + downbeat as f64 * period,
        period,
        best_bpm,
    ))
}

/// Find the phrases of a track on the beats the analysis found (`head_beat` to `tail_beat`), the ones the blends are
/// laid on anyway: only the bar line is chosen among the beats (the one the kick falls on hardest), and the sections are
/// told apart by the energy of the bars. `None` when the analysis has no beats to go by.
pub fn analyze_on(pcm: &[f32], analysis: &super::TrackAnalysis) -> Option<Phrase> {
    let hops = pcm.len() / 2 / HOP;
    if !(analysis.bpm.is_finite() && analysis.bpm > 40.0 && analysis.bpm < 240.0) || hops < 2_000 {
        return None;
    }
    let nominal = RATE * 60.0 / analysis.bpm;
    let beats = ((analysis.tail_beat - analysis.head_beat) / nominal).round();
    if beats < 8.0 {
        return None;
    }
    // The beats between the first and the last one that were found, as far apart as they were found to be.
    let period_samples = (analysis.tail_beat - analysis.head_beat) / beats;
    let period = period_samples / HOP as f64;
    let head = analysis.head_beat / HOP as f64;
    let (full, flux) = hop_energies(pcm);
    let mut sums = [0.0; 4];
    let (mut beat, mut at) = (0usize, head - (head / period).floor() * period);
    while (at as usize) < flux.len() {
        // The onset falls a little to either side of the beat that was found.
        let near = at.round() as isize;
        let strongest = (-2..=2)
            .filter_map(|d| flux.get((near + d).max(0) as usize))
            .fold(0.0, |a: f64, b| a.max(*b));
        sums[beat % 4] += strongest;
        beat += 1;
        at += period;
    }
    let downbeat = (0..4)
        .max_by(|&a, &b| sums[a].total_cmp(&sums[b]))
        .unwrap_or(0);
    let first = head - (head / period).floor() * period + downbeat as f64 * period;
    Some(phrase_on(
        &full,
        first,
        period,
        60.0 * RATE / period_samples,
    ))
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
fn sections(energy: &[f64], first_bar: f64, bar_samples: f64) -> Vec<Marker> {
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
    let at = |bar: usize| (first_bar + bar as f64 * bar_samples).round() as i64;
    // Two marks a few bars apart are one place.
    let mut mark = |kind: Kind, bar: usize| {
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
                mark(Kind::Start, 0);
            } else if !seen_full {
                // The first full part after a thin beginning.
                mark(Kind::Intro, bar);
            } else {
                mark(Kind::Drop, bar);
            }
            seen_full = true;
        } else if bars >= MIN_BARS {
            if index == last && seen_full {
                mark(Kind::Outro, bar);
            } else if index != 0 && seen_full {
                mark(Kind::Breakdown, bar);
            }
        } else if index == last && seen_full && bars >= 2 {
            mark(Kind::Outro, bar);
        }
    }
    if marks.first().is_none_or(|m| m.kind != Kind::Start) && energy[0] / top >= THIN {
        marks.insert(
            0,
            Marker {
                at: at(0),
                kind: Kind::Start,
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
    pub(crate) fn track(first: f64) -> Vec<f32> {
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

    fn near(phrase: &Phrase, kind: Kind, seconds: f64) -> bool {
        phrase
            .markers
            .iter()
            .any(|m| m.kind == kind && (m.at as f64 / RATE - seconds).abs() < 0.15)
    }

    #[test]
    fn the_grid_follows_the_kicks_even_when_the_bpm_is_a_little_off() {
        let phrase = analyze(&track(0.13), 120.6).unwrap();
        assert!((phrase.bpm - 120.0).abs() < 0.3, "bpm {}", phrase.bpm);
        let offset = (phrase.first_bar / RATE) % 2.0;
        assert!((offset - 0.13).abs() < 0.03, "offset {offset}");
        assert!((phrase.bar / RATE - 2.0).abs() < 0.01);
    }

    #[test]
    fn the_sections_are_found() {
        let phrase = analyze(&track(0.13), 120.0).unwrap();
        // The bars begin at 0.13 s and last 2 s.
        assert!(
            near(&phrase, Kind::Intro, 0.13 + 16.0),
            "{:?}",
            phrase.markers
        );
        assert!(
            near(&phrase, Kind::Breakdown, 0.13 + 48.0),
            "{:?}",
            phrase.markers
        );
        assert!(
            near(&phrase, Kind::Drop, 0.13 + 64.0),
            "{:?}",
            phrase.markers
        );
        assert!(
            near(&phrase, Kind::Outro, 0.13 + 96.0),
            "{:?}",
            phrase.markers
        );
    }

    #[test]
    fn the_phrases_follow_the_beats_the_analysis_found() {
        let pcm = track(0.13);
        // The analysis found the second beat of the first bar and the last beat before the end: the grid goes through them,
        // and the bar line is still found on the one.
        let head = (0.13 + 0.5) * RATE;
        let tail = (0.13 + 0.5 + 187.0 * 0.5) * RATE;
        let analysis = crate::mix::TrackAnalysis {
            bpm: 120.0,
            head_beat: head,
            tail_beat: tail,
            start: 0,
            end: (pcm.len() / 2) as i64,
            loudness_db: -9.0,
            intro_end: -1,
            outro_start: -1,
            key: 0,
        };
        let phrase = analyze_on(&pcm, &analysis).unwrap();
        let offset = (phrase.first_bar / RATE) % 2.0;
        assert!((offset - 0.13).abs() < 0.03, "offset {offset}");
        assert!((phrase.bar / RATE - 2.0).abs() < 0.001);
        assert!(
            near(&phrase, Kind::Breakdown, 0.13 + 48.0),
            "{:?}",
            phrase.markers
        );
        assert!(
            near(&phrase, Kind::Outro, 0.13 + 96.0),
            "{:?}",
            phrase.markers
        );
    }

    #[test]
    fn bar_lines_are_on_the_grid() {
        let phrase = analyze(&track(0.13), 120.0).unwrap();
        let line = phrase.bar_at_or_after(10.0 * RATE);
        assert!(((line - phrase.first_bar) / phrase.bar).fract().abs() < 1e-6);
        assert!(line >= 10.0 * RATE && line < 10.0 * RATE + phrase.bar);
        assert!(phrase.bar_at_or_before(10.0 * RATE) <= 10.0 * RATE);
    }

    #[test]
    fn section_edges_restart_the_phrase_grid() {
        let p = Phrase {
            bpm: 120.0,
            first_bar: 0.0,
            bar: 2.0 * RATE,
            markers: vec![Marker {
                at: (20.0 * RATE) as i64,
                kind: Kind::Breakdown,
                bar: 10,
            }],
        };
        let seconds: Vec<_> = p
            .boundaries(0, (60.0 * RATE) as i64)
            .iter()
            .map(|&at| at as f64 / RATE)
            .collect();
        assert_eq!(seconds, vec![0.0, 16.0, 20.0, 36.0, 52.0]);
    }

    #[test]
    fn no_tempo_no_phrases() {
        assert!(analyze(&track(0.0), 0.0).is_none());
        assert!(analyze(&[0.0; 1000], 120.0).is_none());
    }
}
