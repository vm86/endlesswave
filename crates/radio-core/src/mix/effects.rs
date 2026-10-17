//! Club effects, the limiter and `blend`, the whole of a transition: task 1.9 of docs/rust-port.md.
//! Ported from `dj_mix.py`: `riser`, `impact`, `echo_out`, `limit`, `add_drop`, `applied_blend`, `blend`.
//!
//! The noise of the riser and of the boom comes from a [`NoiseSource`], so that a test can give the very
//! numbers the Python radio drew.

use super::{
    CHANNELS, SAMPLE_RATE,
    blend::{self, Spectrum, linspace, sweep},
    plan::Transition,
};
use std::f64::consts::PI;

const SWEEP_TOP: f32 = 9.0; // the last point of the axis of SWEEP_CUTOFFS_HZ
/// Club effects: loudness is relative to the blend they are added to.
const RISER_BEATS: f64 = 8.0;
const RISER_FLOOR_HZ: f64 = 400.0;
const RISER_GAIN: f64 = 0.8;
const IMPACT_SECONDS: f64 = 1.2;
const IMPACT_GAIN: f64 = 2.0;
const IMPACT_PEAK: f64 = 0.6;
/// Echo-out for blends without a shared beat grid: where the outgoing track is cut (share of the blend), the
/// echo spacing and decay.
const ECHO_CUT_AT: f64 = 0.55;
const ECHO_DELAY_SECONDS: f64 = 0.375;
const ECHO_REPEATS: i64 = 6;
const ECHO_FEEDBACK: f64 = 0.6;
const ECHO_DARKEN_HZ: f64 = 4000.0;
const CUT_FADE_SECONDS: f64 = 0.01;
/// Shorter blends are plain crossfades; a swap or sweep needs room.
const MIN_FILTERED_BLEND_SECONDS: f64 = 2.0;

/// The blend is limited so that its peaks stay under this; the gain is worked out per block of `LIMIT_BLOCK`
/// samples, is already down `LIMIT_AHEAD` blocks before a peak and comes back up over about `1 / LIMIT_RELEASE`
/// blocks.
pub const LIMIT_CEILING: f32 = 0.97;
const LIMIT_BLOCK: usize = 64;
const LIMIT_AHEAD: usize = 4;
const LIMIT_RELEASE: f64 = 0.02;

/// Where the noise comes from.
pub trait NoiseSource {
    /// `count` numbers of the normal distribution, mean 0, deviation 1.
    fn normal(&mut self, count: usize) -> Vec<f64>;
}

/// Random noise (`rand`): for the radio itself.
pub struct RandomNoise<R: rand::Rng>(pub R);

impl RandomNoise<rand::rngs::StdRng> {
    /// Seeded from the system.
    pub fn from_entropy() -> Self {
        Self(rand::make_rng::<rand::rngs::StdRng>())
    }
}

impl<R: rand::Rng> NoiseSource for RandomNoise<R> {
    fn normal(&mut self, count: usize) -> Vec<f64> {
        use rand_distr::{Distribution, StandardNormal};
        (0..count).map(|_| StandardNormal.sample(&mut self.0)).collect()
    }
}

/// Python's `round()` of a float: halves go to the even number.
fn round(value: f64) -> i64 {
    value.round_ties_even() as i64
}

/// White noise that gets brighter and louder, peak 1 (`frames` stereo frames).
pub fn riser(frames: usize, noise: &mut dyn NoiseSource) -> Vec<f32> {
    let noise: Vec<f32> = noise.normal(frames * CHANNELS).into_iter().map(|x| x as f32).collect();
    let position = linspace(f64::from(SWEEP_TOP), frames);
    let mut bright = sweep(&noise, &position, false);
    let low = Spectrum::new(&bright).lowpass(RISER_FLOOR_HZ);
    for (b, l) in bright.iter_mut().zip(&low) {
        *b -= l;
    }
    let envelope: Vec<f32> = linspace(1.0, frames).into_iter().map(|e| e.powf(2.5)).collect();
    for (frame, &e) in envelope.iter().enumerate() {
        for c in 0..CHANNELS {
            bright[frame * CHANNELS + c] *= e;
        }
    }
    // The riser is cut off on the drop; fade the last few ms to avoid a click.
    let fade = frames.min(round(CUT_FADE_SECONDS * f64::from(SAMPLE_RATE)) as usize);
    let fall = linspace_between(1.0, 0.0, fade);
    for (n, &g) in fall.iter().enumerate() {
        for c in 0..CHANNELS {
            bright[(frames - fade + n) * CHANNELS + c] *= g;
        }
    }
    let peak = bright.iter().fold(0.0f32, |top, x| top.max(x.abs()));
    if peak != 0.0 {
        for x in &mut bright {
            *x /= peak;
        }
    }
    bright
}

/// `np.linspace(start, end, count, dtype=float32)`.
fn linspace_between(start: f64, end: f64, count: usize) -> Vec<f32> {
    match count {
        0 => Vec::new(),
        1 => vec![start as f32],
        _ => (0..count).map(|i| (start + (end - start) * i as f64 / (count - 1) as f64) as f32).collect(),
    }
}

/// A deep boom: a sine falling from ~110 to 40 Hz, plus a click (`frames` stereo frames at most).
pub fn impact(frames: usize, noise: &mut dyn NoiseSource) -> Vec<f32> {
    let count = frames.min(round(IMPACT_SECONDS * f64::from(SAMPLE_RATE)) as usize);
    let click = noise.normal(count);
    let rate = f64::from(SAMPLE_RATE);
    let mut total = 0.0;
    let mono: Vec<f64> = (0..count)
        .map(|i| {
            let t = i as f64 / rate;
            total += 40.0 + 70.0 * (-t * 18.0).exp();
            let phase = 2.0 * PI * total / rate;
            phase.sin() * (-t * 4.5).exp() + click[i] * (-t * 60.0).exp() * 0.3
        })
        .collect();
    let peak = mono.iter().fold(0.0f64, |top, x| top.max(x.abs())).max(1e-9);
    mono.iter().flat_map(|&m| [(m / peak) as f32; CHANNELS]).collect()
}

/// Cut the track at `cut` and let its last slice echo on, fading. Before the cut nothing changes. The last slice
/// (one echo delay long) is then repeated, each time quieter and darker, as if the track was thrown into a delay
/// unit at the moment its fader went down.
pub fn echo_out(pcm: &[f32], cut: usize, level: f64) -> Vec<f32> {
    let frames = pcm.len() / CHANNELS;
    let delay = round(ECHO_DELAY_SECONDS * f64::from(SAMPLE_RATE)) as usize;
    let cut = cut.max(delay);
    let edge = (delay / 2).min(round(CUT_FADE_SECONDS * f64::from(SAMPLE_RATE)) as usize);
    let (rise, fall) = (linspace_between(0.0, 1.0, edge), linspace_between(1.0, 0.0, edge));

    let mut slice = pcm[(cut - delay) * CHANNELS..cut * CHANNELS].to_vec();
    for n in 0..edge {
        for c in 0..CHANNELS {
            slice[n * CHANNELS + c] *= rise[n];
            slice[(delay - edge + n) * CHANNELS + c] *= fall[n];
        }
    }

    let mut result = pcm.to_vec();
    result[cut * CHANNELS..].fill(0.0);
    for n in 0..edge {
        for c in 0..CHANNELS {
            result[(cut - edge + n) * CHANNELS + c] *= fall[n];
        }
    }

    let mut tail = vec![0.0f32; pcm.len()];
    for repeat in 1..=ECHO_REPEATS {
        let start = cut + (repeat as usize - 1) * delay;
        if start >= frames {
            break;
        }
        let end = frames.min(start + delay);
        let gain = (ECHO_FEEDBACK * level).powi(repeat as i32) as f32;
        for n in 0..(end - start) * CHANNELS {
            tail[start * CHANNELS + n] = slice[n] * gain;
        }
    }
    let dark = Spectrum::new(&tail).lowpass(ECHO_DARKEN_HZ);
    for (r, d) in result.iter_mut().zip(&dark) {
        *r += d;
    }
    result
}

/// Keep the peaks of `pcm` under the ceiling by dipping its gain smoothly. Two loud tracks, a riser and a boom
/// add up to more than the 0 dBFS that fits in the encoder, and anything beyond is clipped, which is heard as
/// crackle. The gain goes down ahead of such a peak and comes back after it, the same for both channels.
pub fn limit(pcm: &[f32], ceiling: f32) -> Vec<f32> {
    let frames = pcm.len() / CHANNELS;
    if frames == 0 || pcm.iter().fold(0.0f32, |top, x| top.max(x.abs())) <= ceiling {
        return pcm.to_vec();
    }
    let count = frames.div_ceil(LIMIT_BLOCK);
    let mut peak = vec![0.0f32; count * LIMIT_BLOCK];
    for (slot, frame) in peak.iter_mut().zip(pcm.chunks_exact(CHANNELS)) {
        *slot = frame.iter().fold(0.0f32, |top, x| top.max(x.abs()));
    }
    let need: Vec<f32> = peak
        .chunks_exact(LIMIT_BLOCK)
        .map(|block| (ceiling / block.iter().fold(0.0f32, |top, &x| top.max(x)).max(1e-9)).min(1.0))
        .collect();

    // The gain is already low where a peak is about to come: a minimum over the blocks around, smoothed by a
    // moving average of the same width.
    let width = 2 * LIMIT_AHEAD + 1;
    let pad = |values: &[f32]| -> Vec<f32> {
        let mut padded = vec![1.0f32; LIMIT_AHEAD];
        padded.extend_from_slice(values);
        padded.extend(std::iter::repeat_n(1.0f32, LIMIT_AHEAD));
        padded
    };
    let padded = pad(&need);
    let held: Vec<f32> = padded.windows(width).map(|w| w.iter().fold(f32::INFINITY, |low, &x| low.min(x))).collect();
    let padded = pad(&held);
    let weight = 1.0 / width as f64;
    let mut gain: Vec<f64> = padded.windows(width).map(|w| w.iter().map(|&x| f64::from(x) * weight).sum()).collect();
    // And back up slowly, never above what the peaks allow.
    for index in 1..count {
        let recovered = gain[index - 1] + LIMIT_RELEASE * (1.0 - gain[index - 1]);
        gain[index] = gain[index].min(recovered);
    }

    // np.interp(arange(frames), centers, gain): linear between the centres of the blocks, flat beyond.
    let center = |i: usize| (i as f64 + 0.5) * LIMIT_BLOCK as f64;
    let mut limited = pcm.to_vec();
    let mut block = 0;
    for frame in 0..frames {
        let x = frame as f64;
        while block + 1 < count && center(block + 1) <= x {
            block += 1;
        }
        let per_sample = if x <= center(0) {
            gain[0]
        } else if block + 1 >= count {
            gain[count - 1]
        } else {
            let (x0, x1) = (center(block), center(block + 1));
            gain[block] + (gain[block + 1] - gain[block]) * ((x - x0) / (x1 - x0))
        } as f32;
        for c in 0..CHANNELS {
            // What the smoothing can't catch, to the last sample.
            limited[frame * CHANNELS + c] = (pcm[frame * CHANNELS + c] * per_sample).clamp(-1.0, 1.0);
        }
    }
    limited
}

fn rms(pcm: &[f32]) -> f64 {
    (pcm.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / pcm.len() as f64 + 1e-12).sqrt()
}

/// Add a riser that builds up to the drop and a boom on it.
pub fn add_drop(blended: &[f32], drop: i64, beat_period: f64, level: f64, noise: &mut dyn NoiseSource) -> Vec<f32> {
    let frames = blended.len() / CHANNELS;
    let loudness = rms(blended);
    let length = drop.min(round(RISER_BEATS * beat_period));
    let mut result = blended.to_vec();
    if length as f64 > MIN_FILTERED_BLEND_SECONDS * f64::from(SAMPLE_RATE) / 4.0 {
        let rise = riser(length as usize, noise);
        let gain = (RISER_GAIN * level * loudness) as f32;
        let from = (drop - length) as usize * CHANNELS;
        for (r, x) in result[from..].iter_mut().zip(&rise) {
            *r += x * gain;
        }
    }
    if drop >= frames as i64 {
        return result;
    }
    let boom = impact(frames - drop as usize, noise);
    let gain = (IMPACT_GAIN * level * loudness).min(IMPACT_PEAK) as f32;
    let from = drop as usize * CHANNELS;
    for (r, x) in result[from..].iter_mut().zip(&boom) {
        *r += x * gain;
    }
    result
}

/// What a blend is dressed up with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    None,
    /// A riser builds up to the drop and a boom lands on it.
    Drop,
    /// The outgoing track is echoed out.
    Echo,
}

impl Effect {
    /// The name the API and the statistics give it.
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Drop => "drop",
            Self::Echo => "echo",
        }
    }
}

/// What a blend really is, which can differ from what was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Applied {
    pub kind: Transition,
    pub effect: Effect,
}

/// Say what [`blend`] makes of the pieces. A piece too short for a filtered blend is just crossfaded, with no
/// effects; otherwise the effects follow `effects` and the beats (a drop with a shared grid, else an echo).
pub fn applied_blend(kind: Transition, frames: usize, beat_period: f64, effects: f64) -> Applied {
    if (frames as f64) < MIN_FILTERED_BLEND_SECONDS * f64::from(SAMPLE_RATE) {
        return Applied { kind: Transition::Crossfade, effect: Effect::None };
    }
    if effects <= 0.0 {
        return Applied { kind, effect: Effect::None };
    }
    Applied { kind, effect: if beat_period != 0.0 { Effect::Drop } else { Effect::Echo } }
}

/// Blend two equally long pieces the given way. `effects` (0..1) dresses the transition up like in a club: with
/// a shared beat grid a riser builds up to the drop and a boom lands on it; without one the outgoing track is
/// echoed out instead, as the beats can't be lined up. `beat_grid` says where the beats are when the tempo
/// glides over the blend (`beat_period` is then their mean distance).
pub fn blend(
    kind: Transition,
    outgoing: &[f32],
    incoming: &[f32],
    beat_period: f64,
    effects: f64,
    noise: &mut dyn NoiseSource,
    beat_grid: Option<&[f64]>,
) -> Vec<f32> {
    let frames = outgoing.len() / CHANNELS;
    let used = applied_blend(kind, frames, beat_period, effects);
    if used.kind != kind {
        return blend::crossfade(outgoing, incoming);
    }

    let echoed;
    let outgoing = if used.effect == Effect::Echo {
        echoed = echo_out(outgoing, round(frames as f64 * ECHO_CUT_AT) as usize, effects);
        &echoed[..]
    } else {
        outgoing
    };

    let swap = (beat_period != 0.0).then(|| blend::swap_point(frames, beat_period, beat_grid));
    let blended = match kind {
        Transition::BassSwap => blend::bass_swap(outgoing, incoming, beat_period, swap),
        Transition::EqBlend => blend::eq_blend(outgoing, incoming, beat_period, swap),
        Transition::FilterSweep => blend::filter_sweep(outgoing, incoming),
        Transition::Crossfade => blend::crossfade(outgoing, incoming),
    };

    if used.effect != Effect::Drop {
        return blended;
    }
    let mut period = beat_period;
    let drop = match (swap, beat_grid) {
        (Some(swap), _) if matches!(kind, Transition::BassSwap | Transition::EqBlend) => round(swap.0),
        (_, Some(grid)) if grid.len() > 1 => {
            // The last beat of the blend, so that the boom has room.
            period = grid[grid.len() - 1] - grid[grid.len() - 2];
            grid[grid.len() - 2] as i64
        }
        _ => (((frames as f64 / beat_period).ceil() - 1.0) * beat_period) as i64,
    };
    add_drop(&blended, drop, period, effects, noise)
}
