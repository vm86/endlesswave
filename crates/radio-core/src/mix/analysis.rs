//! The tempo, the beats, the audible range and the sections of a track (tasks 1.3 of docs/rust-port.md).
//! Ported from `dj_mix.py`: `onset_envelope`, `estimate_tempo`, `refine_tempo`, `find_beat`,
//! `audible_range`, `find_sections`, `_snap` and the part of `analyze` that uses them.

use super::{CHANNELS, SAMPLE_RATE};
use realfft::RealFftPlanner;
use std::f64::consts::PI;

const FRAME: usize = 1024;
const HOP: usize = 512;
const FRAMES_PER_SECOND: f64 = SAMPLE_RATE as f64 / HOP as f64;

const MIN_BPM: f64 = 70.0;
const MAX_BPM: f64 = 180.0;
const PREFERRED_BPM: f64 = 120.0;
/// Reject a tempo whose autocorrelation peak is weaker than this.
const MIN_TEMPO_CONFIDENCE: f64 = 0.05;
/// Samples quieter than this (about -40 dBFS) count as silence.
const SILENCE_LEVEL: f32 = 0.01;
/// Length of the stretch of a track the beat position is measured on.
const BEAT_WINDOW_SECONDS: f64 = 30.0;
/// A bar is this many beats; the loudness is looked at one bar at a time.
const BEATS_PER_BAR: f64 = 4.0;
const FALLBACK_BAR_SECONDS: f64 = 2.0;
const MIN_BARS_FOR_STRUCTURE: usize = 8;
/// Bars quieter than the median bar by this much belong to an intro/outro.
const SECTION_THRESHOLD_DB: f64 = 4.0;

/// Spectral flux per hop: how much new sound appears at that moment (the slow trend taken away).
///
/// The radio's version fails on a track of 2048 to about 23000 samples (the trend is shorter than the
/// flux); here the trend is as long as the flux whatever the length.
pub fn onset_envelope(mono: &[f32]) -> Vec<f32> {
    if mono.len() < FRAME {
        return Vec::new();
    }
    let count = (mono.len() - FRAME) / HOP + 1;
    // np.hanning: symmetric.
    let window: Vec<f32> = (0..FRAME)
        .map(|n| (0.5 - 0.5 * (2.0 * PI * n as f64 / (FRAME - 1) as f64).cos()) as f32)
        .collect();
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(FRAME);
    let mut input = fft.make_input_vec();
    let mut output = fft.make_output_vec();
    let mut scratch = fft.make_scratch_vec();
    let mut previous = vec![0.0f32; FRAME / 2 + 1];
    let mut flux = vec![0.0f32; count];
    for (i, flux_i) in flux.iter_mut().enumerate() {
        let frame = &mono[i * HOP..i * HOP + FRAME];
        for ((x, &s), &w) in input.iter_mut().zip(frame).zip(&window) {
            *x = s * w;
        }
        fft.process_with_scratch(&mut input, &mut output, &mut scratch)
            .expect("sizes are right");
        let mut rise = 0.0f64;
        for (bin, last) in output.iter().zip(previous.iter_mut()) {
            let now = (10.0 * bin.norm()).ln_1p();
            if i > 0 {
                rise += f64::from((now - *last).max(0.0));
            }
            *last = now;
        }
        *flux_i = rise as f32;
    }

    // Drop the slow trend so that only the peaks remain: a box of `kernel` frames, zeros beyond the ends
    // (np.convolve(..., mode="same")).
    let kernel = (FRAMES_PER_SECOND / 2.0) as usize;
    let mut sums = vec![0.0f64; count + 1];
    for (i, &f) in flux.iter().enumerate() {
        sums[i + 1] = sums[i] + f64::from(f);
    }
    let before = (kernel - 1) / 2;
    let after = kernel / 2;
    (0..count)
        .map(|i| {
            let from = i.saturating_sub(before);
            let to = (i + after + 1).min(count);
            let trend = (sums[to] - sums[from]) / kernel as f64;
            (f64::from(flux[i]) - trend).max(0.0) as f32
        })
        .collect()
}

/// Tempo in BPM from the autocorrelation of the onset envelope, 0 when there is none.
pub fn estimate_tempo(flux: &[f32]) -> f64 {
    let count = flux.len();
    let min_lag = (FRAMES_PER_SECOND * 60.0 / MAX_BPM) as usize;
    let max_lag = (FRAMES_PER_SECOND * 60.0 / MIN_BPM) as usize;
    if count < 4 * max_lag || flux.iter().all(|&f| f == 0.0) {
        return 0.0;
    }

    // Without the mean even noise would correlate with itself.
    let mean = flux.iter().map(|&f| f64::from(f)).sum::<f64>() / count as f64;
    let size = 2 * count;
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(size);
    let inverse = planner.plan_fft_inverse(size);
    let mut signal = forward.make_input_vec();
    for (x, &f) in signal.iter_mut().zip(flux) {
        *x = f64::from(f) - mean;
    }
    let mut spectrum = forward.make_output_vec();
    forward
        .process(&mut signal, &mut spectrum)
        .expect("sizes are right");
    for bin in &mut spectrum {
        *bin = (bin.norm_sqr()).into();
    }
    let mut correlation = inverse.make_output_vec();
    inverse
        .process(&mut spectrum, &mut correlation)
        .expect("sizes are right");
    // realfft does not scale the inverse: only the ratio to the first lag matters.
    let zero = correlation[0];
    for c in &mut correlation {
        *c /= zero;
    }

    let mut best: Option<(usize, f64)> = None;
    for lag in min_lag..=max_lag {
        let bpm = 60.0 * FRAMES_PER_SECOND / lag as f64;
        let prefer = (-0.5 * (bpm / PREFERRED_BPM).log2().powi(2)).exp();
        let score = (correlation[lag] + 0.5 * correlation[lag * 2]) * prefer;
        // The first of equal scores wins, as np.argmax says.
        if best.is_none_or(|(_, top)| score > top) {
            best = Some((lag, score));
        }
    }
    let (lag, _) = best.expect("the range of lags is not empty");
    if correlation[lag] < MIN_TEMPO_CONFIDENCE {
        return 0.0;
    }

    // Parabolic interpolation around the peak for a fractional lag.
    let mut fractional = lag as f64;
    if lag > min_lag && lag < max_lag {
        let (left, mid, right) = (correlation[lag - 1], correlation[lag], correlation[lag + 1]);
        let denominator = left - 2.0 * mid + right;
        if denominator < 0.0 {
            fractional += 0.5 * (left - right) / denominator;
        }
    }
    60.0 * FRAMES_PER_SECOND / fractional
}

/// Onset energy found on a beat grid, for every grid phase: `(offsets, scores)`.
fn comb_scores(flux: &[f32], period: f64) -> (Vec<f64>, Vec<f64>) {
    let offsets: Vec<f64> = (0..)
        .map(|i| f64::from(i) * 0.5)
        .take_while(|&o| o < period)
        .collect();
    let beats = ((flux.len() as f64 / period) as i64 - 1).max(0);
    let scores = offsets
        .iter()
        .map(|&offset| {
            (0..beats)
                .map(|beat| f64::from(flux[(offset + period * beat as f64) as usize]))
                .sum()
        })
        .collect();
    (offsets, scores)
}

/// The index of the first greatest value, as np.argmax says.
fn argmax(values: &[f64]) -> usize {
    let mut best = 0;
    for (i, &v) in values.iter().enumerate() {
        if v > values[best] {
            best = i;
        }
    }
    best
}

/// Fine tuning: the tempo whose beat grid fits the onsets best. The autocorrelation is precise to about a
/// percent, which over a whole track is a drift of seconds.
pub fn refine_tempo(flux: &[f32], bpm: f64) -> f64 {
    let (mut best_bpm, mut best_score) = (bpm, -1.0);
    for step in 0..81 {
        // np.linspace(-0.02, 0.02, 81)
        let shift = -0.02 + 0.04 * f64::from(step) / 80.0;
        let candidate = bpm * (1.0 + shift);
        let (_, scores) = comb_scores(flux, 60.0 * FRAMES_PER_SECOND / candidate);
        let top = scores.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if top > best_score {
            (best_bpm, best_score) = (candidate, top);
        }
    }
    best_bpm
}

/// Position, in samples, of a beat inside the given range of frames.
///
/// The radio's version takes a negative start (a track shorter than the window) as an index from the end of
/// the flux and gets a nonsense; here it is the start of the flux.
pub fn find_beat(flux: &[f32], bpm: f64, first_frame: i64, last_frame: usize) -> f64 {
    let first = first_frame.max(0) as usize;
    let period = 60.0 * FRAMES_PER_SECOND / bpm;
    let (offsets, scores) = comb_scores(
        &flux[first.min(last_frame)..last_frame.min(flux.len())],
        period,
    );
    let best = offsets[argmax(&scores)] + first as f64;
    best * HOP as f64 + FRAME as f64 / 2.0
}

/// The first audible sample and one past the last; the whole track when there is no sound.
pub fn audible_range(pcm: &[f32]) -> (usize, usize) {
    let loud = |frame: &[f32]| frame.iter().any(|s| s.abs() > SILENCE_LEVEL);
    let frames = pcm.len() / CHANNELS;
    let first = pcm.chunks_exact(CHANNELS).position(loud);
    match first {
        None => (0, frames),
        Some(first) => {
            let last = pcm
                .chunks_exact(CHANNELS)
                .rposition(loud)
                .expect("there is a first one");
            (first, last + 1)
        }
    }
}

/// Where the intro ends and the outro starts, from the loudness: the track is cut into bars, the intro is
/// everything before the first bar as loud as the track usually is, the outro everything after the last
/// one. `None` when the track is too short to tell.
pub fn find_sections(mono: &[f32], start: usize, bar: usize) -> Option<(usize, usize)> {
    let count = mono.len() / bar;
    if count < MIN_BARS_FOR_STRUCTURE {
        return None;
    }
    let levels: Vec<f64> = mono[..count * bar]
        .chunks_exact(bar)
        .map(|b| {
            10.0 * (b.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / bar as f64 + 1e-12)
                .log10()
        })
        .collect();
    let mut sorted = levels.clone();
    sorted.sort_by(f64::total_cmp);
    let median = if count % 2 == 1 {
        sorted[count / 2]
    } else {
        (sorted[count / 2 - 1] + sorted[count / 2]) / 2.0
    };
    let loud = |&&level: &&f64| level >= median - SECTION_THRESHOLD_DB;
    let first = levels.iter().position(|l| loud(&l))?;
    let last = levels.iter().rposition(|l| loud(&l))?;
    Some((start + first * bar, start + (last + 1) * bar))
}

/// Move a position onto the bar grid, later (`up`) or earlier.
fn snap(position: i64, grid_beat: f64, bar: f64, up: bool) -> i64 {
    let bars = (position as f64 - grid_beat) / bar;
    let whole = if up { bars.ceil() } else { bars.floor() };
    // Python's round(): halves go to the even number.
    (grid_beat + whole * bar).round_ties_even() as i64
}

/// What the analysis finds without the loudness and the key.
#[derive(Debug, Clone, PartialEq)]
pub struct Rhythm {
    pub bpm: f64,
    pub head_beat: f64,
    pub tail_beat: f64,
    pub start: usize,
    pub end: usize,
    pub intro_end: i64,
    pub outro_start: i64,
}

/// The rhythm and the structure of a stereo, interleaved track (the part of `analyze` of `dj_mix.py`
/// that does not need the key or the loudness).
pub fn rhythm(pcm: &[f32]) -> Rhythm {
    let (start, end) = audible_range(pcm);
    let mono: Vec<f32> = pcm[start * CHANNELS..end * CHANNELS]
        .chunks_exact(CHANNELS)
        .map(|f| (f[0] + f[1]) / 2.0)
        .collect();

    let (mut bpm, mut head_beat, mut tail_beat) = (0.0, 0.0, 0.0);
    if mono.len() > 2 * FRAME {
        let flux = onset_envelope(&mono);
        bpm = estimate_tempo(&flux);
        if bpm != 0.0 {
            bpm = refine_tempo(&flux, bpm);
            // The grid is only known to be exact near where it was measured, so measure it where the
            // tracks will be blended.
            let edge = (BEAT_WINDOW_SECONDS * FRAMES_PER_SECOND) as usize;
            head_beat = start as f64 + find_beat(&flux, bpm, 0, edge);
            tail_beat =
                start as f64 + find_beat(&flux, bpm, flux.len() as i64 - edge as i64, flux.len());
        }
    }

    let bar = if bpm != 0.0 {
        BEATS_PER_BAR * SAMPLE_RATE as f64 * 60.0 / bpm
    } else {
        0.0
    };
    let bar_samples = (if bar != 0.0 {
        bar
    } else {
        FALLBACK_BAR_SECONDS * SAMPLE_RATE as f64
    })
    .round_ties_even() as usize;
    let (mut intro_end, mut outro_start) = match find_sections(&mono, start, bar_samples) {
        Some((intro, outro)) => (intro as i64, outro as i64),
        None => (-1, -1),
    };
    if bpm != 0.0 && intro_end >= 0 {
        // A transition should land on a bar line of the track it is in.
        intro_end = snap(intro_end, head_beat, bar, true).min(end as i64);
        outro_start = snap(outro_start, tail_beat, bar, false).max(start as i64);
        if outro_start <= intro_end {
            intro_end = -1;
            outro_start = -1;
        }
    }
    Rhythm {
        bpm,
        head_beat,
        tail_beat,
        start,
        end,
        intro_end,
        outro_start,
    }
}
