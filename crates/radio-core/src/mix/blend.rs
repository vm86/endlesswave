//! The blends of two equally long pieces, task 1.7 of docs/rust-port.md. Ported from `dj_mix.py`:
//! `crossfade`, `_lowpass`, `_spectrum`, `swap_point`, `bass_swap`, `_handover`, `_fade_angle`, `eq_blend`,
//! `_sweep`, `filter_sweep`.
//!
//! A piece is stereo, interleaved (`[left, right, left, right, ...]`), as everywhere in this crate.

use super::{CHANNELS, SAMPLE_RATE};
use realfft::{RealFftPlanner, num_complex::Complex};
use std::f32::consts::FRAC_PI_2 as HALF_PI_32;
use std::f64::consts::FRAC_PI_2 as HALF_PI;

/// Below the crossover the two tracks never play together (bass swap).
const CROSSOVER_HZ: f64 = 200.0;
/// The three-band blend splits the sound at `CROSSOVER_HZ` and at this.
const EQ_HIGH_HZ: f64 = 2500.0;
/// Where in the blend (0..1) the bands change hands. The mids go first and the bass in the middle (see
/// `swap_point`); the highs take most of the blend.
const EQ_MIDS_SWAP: (f32, f32) = (0.20, 0.50);
const EQ_HIGHS_SWAP: (f32, f32) = (0.15, 0.85);
/// Cutoffs of the filtered copies a sweep moves between, low to high.
const SWEEP_CUTOFFS_HZ: [f64; 10] = [150.0, 250.0, 400.0, 650.0, 1000.0, 1600.0, 2600.0, 4200.0, 7000.0, 12000.0];
/// Share of the blend after which the sweeps are done.
const SWEEP_DONE_AT: f32 = 0.85;
/// Spectrum padding, so that the circular FFT filter doesn't wrap around.
const FFT_PADDING: usize = 1 << 14;

/// `np.linspace(0, end, count, dtype=float32)`: worked out in double, then narrowed.
fn linspace(end: f64, count: usize) -> Vec<f32> {
    match count {
        0 => Vec::new(),
        1 => vec![0.0],
        _ => (0..count).map(|i| (end * i as f64 / (count - 1) as f64) as f32).collect(),
    }
}

/// Equal-power blend of two equally long pieces.
pub fn crossfade(outgoing: &[f32], incoming: &[f32]) -> Vec<f32> {
    let fade = linspace(HALF_PI, outgoing.len() / CHANNELS);
    let mut blended = Vec::with_capacity(outgoing.len());
    for (frame, &angle) in fade.iter().enumerate() {
        let (out_gain, in_gain) = (angle.cos(), angle.sin());
        for c in 0..CHANNELS {
            blended.push(outgoing[frame * CHANNELS + c] * out_gain + incoming[frame * CHANNELS + c] * in_gain);
        }
    }
    blended
}

/// The spectrum of each channel of a piece, padded with zeros (`_spectrum`).
pub struct Spectrum {
    size: usize,
    frames: usize,
    channels: Vec<Vec<Complex<f32>>>,
}

impl Spectrum {
    pub fn new(pcm: &[f32]) -> Self {
        let frames = pcm.len() / CHANNELS;
        // 1 << (frames + padding - 1).bit_length(): the power of two above frames + padding - 1.
        let size = 1usize << (usize::BITS - (frames + FFT_PADDING - 1).leading_zeros());
        let forward = RealFftPlanner::<f32>::new().plan_fft_forward(size);
        let channels = (0..CHANNELS)
            .map(|c| {
                let mut input = forward.make_input_vec();
                for (x, frame) in input.iter_mut().zip(pcm.chunks_exact(CHANNELS)) {
                    *x = frame[c];
                }
                let mut output = forward.make_output_vec();
                forward.process(&mut input, &mut output).expect("sizes are right");
                output
            })
            .collect();
        Self { size, frames, channels }
    }

    /// Zero-phase low-pass (4th order Butterworth magnitude), stereo, interleaved (`_lowpass`).
    pub fn lowpass(&self, cutoff: f64) -> Vec<f32> {
        let inverse = RealFftPlanner::<f32>::new().plan_fft_inverse(self.size);
        let gain: Vec<f64> = (0..=self.size / 2)
            .map(|k| 1.0 / (1.0 + (k as f64 * f64::from(SAMPLE_RATE) / self.size as f64 / cutoff).powi(8)).sqrt())
            .collect();
        let mut result = vec![0.0f32; self.frames * CHANNELS];
        for (c, spectrum) in self.channels.iter().enumerate() {
            let mut filtered: Vec<Complex<f32>> =
                spectrum.iter().zip(&gain).map(|(bin, &g)| bin * g as f32).collect();
            let mut output = inverse.make_output_vec();
            inverse.process(&mut filtered, &mut output).expect("sizes are right");
            for (frame, &x) in output.iter().take(self.frames).enumerate() {
                result[frame * CHANNELS + c] = x / self.size as f32;
            }
        }
        result
    }
}

/// Where the bass changes hands and how long it takes: `(point, ramp)` in frames. `beat_grid` holds the position
/// of each beat of a blend whose tempo glides, and the bass changes hands on the beat nearest to the middle.
pub fn swap_point(frames: usize, beat_period: f64, beat_grid: Option<&[f64]>) -> (f64, f64) {
    let middle = frames as f64 / 2.0;
    if let Some(grid) = beat_grid.filter(|grid| grid.len() > 1) {
        let mut beat = 0;
        for (i, &position) in grid.iter().enumerate() {
            if (position - middle).abs() < (grid[beat] - middle).abs() {
                beat = i;
            }
        }
        let beat = beat.min(grid.len() - 2);
        return (grid[beat], (grid[beat + 1] - grid[beat]) / 2.0);
    }
    if beat_period == 0.0 {
        return (middle, f64::from(SAMPLE_RATE) / 4.0);
    }
    ((middle / beat_period).round_ties_even() * beat_period, beat_period / 2.0)
}

/// Angle of an equal-power fade that is done in 2 * ramp around `swap` (`_handover`).
fn handover(frames: usize, (point, ramp): (f64, f64)) -> Vec<f32> {
    (0..frames)
        .map(|i| (((i as f64 - (point - ramp)) / (2.0 * ramp)).clamp(0.0, 1.0) * HALF_PI) as f32)
        .collect()
}

/// Angle of an equal-power fade over a part of the blend, 0..1 (`_fade_angle`).
fn fade_angle(frames: usize, window: (f32, f32)) -> Vec<f32> {
    linspace(1.0, frames)
        .into_iter()
        .map(|progress| ((progress - window.0) / (window.1 - window.0)).clamp(0.0, 1.0) * HALF_PI_32)
        .collect()
}

/// `piece * cos(angle)` for every sample of every channel of the frame.
fn gains(angles: &[f32]) -> impl Iterator<Item = (f32, f32)> + '_ {
    angles.iter().map(|a| (a.cos(), a.sin()))
}

/// Blend the highs, hand the bass over in one go in the middle. Two basslines at once turn the low end to mud,
/// so the outgoing bass stays until the swap point and the incoming one only starts there. The swap is on a
/// beat when the beat period is known (or `swap` says where, see `swap_point`).
pub fn bass_swap(outgoing: &[f32], incoming: &[f32], beat_period: f64, swap: Option<(f64, f64)>) -> Vec<f32> {
    let frames = outgoing.len() / CHANNELS;
    let (out_spec, in_spec) = (Spectrum::new(outgoing), Spectrum::new(incoming));
    let (out_low, in_low) = (out_spec.lowpass(CROSSOVER_HZ), in_spec.lowpass(CROSSOVER_HZ));

    let low_angle = handover(frames, swap.unwrap_or_else(|| swap_point(frames, beat_period, None)));
    let high_angle = linspace(HALF_PI, frames);

    let mut result = vec![0.0f32; outgoing.len()];
    for (frame, ((low_cos, low_sin), (high_cos, high_sin))) in gains(&low_angle).zip(gains(&high_angle)).enumerate() {
        for c in 0..CHANNELS {
            let i = frame * CHANNELS + c;
            result[i] = out_low[i] * low_cos
                + in_low[i] * low_sin
                + (outgoing[i] - out_low[i]) * high_cos
                + (incoming[i] - in_low[i]) * high_sin;
        }
    }
    result
}

/// Hand the bass, the mids and the highs over one band at a time, like a DJ with three EQ knobs: the incoming
/// track comes in over the outgoing bass, the mids change hands first (where voices and leads live), then the
/// bass on a beat in the middle, while the highs cross over most of the blend.
pub fn eq_blend(outgoing: &[f32], incoming: &[f32], beat_period: f64, swap: Option<(f64, f64)>) -> Vec<f32> {
    let frames = outgoing.len() / CHANNELS;
    let (out_spec, in_spec) = (Spectrum::new(outgoing), Spectrum::new(incoming));
    let (out_low, in_low) = (out_spec.lowpass(CROSSOVER_HZ), in_spec.lowpass(CROSSOVER_HZ));
    let (out_body, in_body) = (out_spec.lowpass(EQ_HIGH_HZ), in_spec.lowpass(EQ_HIGH_HZ));

    let low = handover(frames, swap.unwrap_or_else(|| swap_point(frames, beat_period, None)));
    let mid = fade_angle(frames, EQ_MIDS_SWAP);
    let high = fade_angle(frames, EQ_HIGHS_SWAP);

    let mut result = vec![0.0f32; outgoing.len()];
    let angles = gains(&low).zip(gains(&mid)).zip(gains(&high));
    for (frame, (((low_cos, low_sin), (mid_cos, mid_sin)), (high_cos, high_sin))) in angles.enumerate() {
        for c in 0..CHANNELS {
            let i = frame * CHANNELS + c;
            result[i] = out_low[i] * low_cos
                + in_low[i] * low_sin
                + (out_body[i] - out_low[i]) * mid_cos
                + (in_body[i] - in_low[i]) * mid_sin
                + (outgoing[i] - out_body[i]) * high_cos
                + (incoming[i] - in_body[i]) * high_sin;
        }
    }
    result
}

/// Filter with a cutoff that moves through `SWEEP_CUTOFFS_HZ` over time (`_sweep`). The filtered copies lie on an
/// axis of `len + 1` points and `position` says, per frame, where on it the output is. For a low-pass the axis
/// runs from the lowest cutoff to the unfiltered track, for a high-pass from the unfiltered track to the
/// highest cutoff. The output blends neighbouring copies, which is far cheaper than a filter whose coefficients
/// change from sample to sample.
fn sweep(pcm: &[f32], position: &[f32], high_pass: bool) -> Vec<f32> {
    let spectrum = Spectrum::new(pcm);
    let levels = SWEEP_CUTOFFS_HZ.len();
    let mut result = vec![0.0f32; pcm.len()];
    for point in 0..=levels {
        let weight: Vec<f32> = position.iter().map(|&p| (1.0 - (p - point as f32).abs()).clamp(0.0, 1.0)).collect();
        if weight.iter().all(|&w| w == 0.0) {
            continue;
        }
        let copy: Vec<f32> = if high_pass {
            if point == 0 {
                pcm.to_vec()
            } else {
                let low = spectrum.lowpass(SWEEP_CUTOFFS_HZ[point - 1]);
                pcm.iter().zip(&low).map(|(&x, &l)| x - l).collect()
            }
        } else if point == levels {
            pcm.to_vec()
        } else {
            spectrum.lowpass(SWEEP_CUTOFFS_HZ[point])
        };
        for (frame, &w) in weight.iter().enumerate() {
            for c in 0..CHANNELS {
                result[frame * CHANNELS + c] += copy[frame * CHANNELS + c] * w;
            }
        }
    }
    result
}

/// Close a low-pass on the outgoing track, open a high-pass on the next. The outgoing track loses its highs and
/// then its body; the incoming one enters as a thin layer of highs and gains its bass last.
pub fn filter_sweep(outgoing: &[f32], incoming: &[f32]) -> Vec<f32> {
    let frames = outgoing.len() / CHANNELS;
    let progress = linspace(1.0, frames);
    // Both sweeps finish a little before the end of the blend, so that the incoming track is fully open when it
    // takes over.
    let position: Vec<f32> =
        progress.iter().map(|&p| SWEEP_CUTOFFS_HZ.len() as f32 * (1.0 - (p / SWEEP_DONE_AT).clamp(0.0, 1.0))).collect();
    let closing = sweep(outgoing, &position, false);
    let opening = sweep(incoming, &position, true);
    let mut blended = Vec::with_capacity(outgoing.len());
    for (frame, &p) in progress.iter().enumerate() {
        let angle = p * HALF_PI_32;
        let (out_gain, in_gain) = (angle.cos(), angle.sin());
        for c in 0..CHANNELS {
            blended.push(closing[frame * CHANNELS + c] * out_gain + opening[frame * CHANNELS + c] * in_gain);
        }
    }
    blended
}
