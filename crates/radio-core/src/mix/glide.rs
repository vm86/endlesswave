//! Stretching the pieces of a blend in time, so that the tempo glides from one track to the other: task 1.8
//! of docs/rust-port.md. Ported from `dj_mix.py`: `find_attacks`, `_best_start`, `_wsola`, `tempo_glide`.
//!
//! The pieces are stereo, interleaved.

use super::{CHANNELS, SAMPLE_RATE};
use std::f64::consts::PI;

/// Below this the tempos are taken for equal and nothing is stretched.
const NEGLIGIBLE_TEMPO_SHIFT: f64 = 0.002;
/// The tempo glide stretches the blend pieces by WSOLA: frames of `GLIDE_WINDOW` samples overlap by half, and
/// each is moved up to a tolerance to continue the previous one without a jump. The search for the best place
/// is done on a signal decimated by `GLIDE_DECIMATION`.
pub const GLIDE_WINDOW: usize = 2048;
const GLIDE_HOP: usize = GLIDE_WINDOW / 2;
const GLIDE_DECIMATION: usize = 4;
/// A wide tolerance keeps tones clean (a narrow one makes them sound phasey and electronic), but it lets the
/// beats of two pieces drift apart by a few ms. So a frame with an attack (a kick, a snare) in it, which has to
/// stay on the beat, may be moved only a little.
const GLIDE_TOLERANCE: usize = 512;
const GLIDE_ATTACK_TOLERANCE: usize = 96;
/// An attack: a block of `GLIDE_BLOCK` samples whose energy is this many times that of the blocks before it.
const GLIDE_BLOCK: usize = 256;
const GLIDE_ATTACK_RATIO: f64 = 4.0;
/// What a piece has to have beyond the span it plays, for the last frames.
pub const GLIDE_MARGIN: usize = GLIDE_WINDOW + GLIDE_TOLERANCE;

/// Flag the blocks of `GLIDE_BLOCK` samples that start an attack.
pub fn find_attacks(mono: &[f32]) -> Vec<bool> {
    let energy: Vec<f64> = mono
        .chunks_exact(GLIDE_BLOCK)
        .map(|block| block.iter().map(|&s| f64::from(s).powi(2)).sum())
        .collect();
    let floor = 1e-4 * GLIDE_BLOCK as f64;
    let peak = energy.iter().copied().fold(0.0, f64::max);
    (0..energy.len())
        .map(|i| {
            // The mean of the four blocks before this one (zeros before the start); the first block is
            // compared with the loudest.
            let before = if i == 0 {
                peak
            } else {
                (i.saturating_sub(4)..i).map(|j| energy[j]).sum::<f64>() / 4.0
            };
            energy[i] > GLIDE_ATTACK_RATIO * before && energy[i] > floor
        })
        .collect()
}

/// Where to read the next frame so it follows the previous one. The frame should start like the signal after
/// the previous frame goes on at `reference`. It is looked for around `nominal`: first on the decimated signal,
/// then at full resolution round the best place.
fn best_start(coarse: &[f32], mono: &[f32], reference: usize, nominal: usize, tolerance: usize) -> usize {
    let (hop, step) = (GLIDE_HOP, GLIDE_DECIMATION);
    let size = hop / step;
    let slice = |from: usize, to: usize| &coarse[from.min(coarse.len())..to.min(coarse.len())];
    let reference_coarse = slice(reference / step, reference / step + size);
    let low = (nominal.saturating_sub(tolerance)) / step;
    let segment = slice(low, low + 2 * tolerance / step + size);
    if reference_coarse.len() < size || segment.len() <= size {
        return nominal;
    }
    // np.correlate(segment, ref, "valid"), divided by the length of each stretch of the segment.
    let mut energy = vec![0.0f64; segment.len() + 1];
    for (i, &s) in segment.iter().enumerate() {
        energy[i + 1] = energy[i] + f64::from(s * s);
    }
    let mut best = (0, f64::NEG_INFINITY);
    for k in 0..=segment.len() - size {
        let corr: f64 = segment[k..k + size].iter().zip(reference_coarse).map(|(&a, &b)| f64::from(a) * f64::from(b)).sum();
        let score = corr / (energy[k + size] - energy[k] + 1e-9).sqrt();
        if score > best.1 {
            best = (k, score);
        }
    }
    let near = (low + best.0) * step;

    let at = |from: usize| &mono[from.min(mono.len())..(from + hop).min(mono.len())];
    let target = at(reference);
    let (mut found, mut found_score) = (nominal, f64::NEG_INFINITY);
    for place in near.saturating_sub(step)..=near + step {
        let candidate = at(place);
        if candidate.len() < hop || target.len() < hop {
            continue;
        }
        let dot: f64 = candidate.iter().zip(target).map(|(&a, &b)| f64::from(a) * f64::from(b)).sum();
        let norm = candidate.iter().map(|&a| f64::from(a).powi(2)).sum::<f64>().sqrt();
        let fit = dot / (norm + 1e-9);
        if fit > found_score {
            (found, found_score) = (place, fit);
        }
    }
    found
}

/// Read `frames` frames from `pcm` at the pace `positions` gives. Waveform similarity overlap-add:
/// `positions[m]` is the place in `pcm` that the m-th frame of the output (it starts at `m * GLIDE_HOP`) is read
/// from. Each frame is moved a little, to where it carries on from the previous one without a jump, and the
/// frames are overlapped. The pitch stays, only the pace changes.
pub fn wsola(pcm: &[f32], positions: &[f64], frames: usize) -> Vec<f32> {
    let (window, hop, pad) = (GLIDE_WINDOW, GLIDE_HOP, GLIDE_MARGIN);
    let mut padded = vec![0.0f32; (pad + pcm.len() / CHANNELS + pad + window) * CHANNELS];
    padded[pad * CHANNELS..pad * CHANNELS + pcm.len()].copy_from_slice(pcm);
    let padded_frames = padded.len() / CHANNELS;
    let mono: Vec<f32> = padded.chunks_exact(CHANNELS).map(|f| (f[0] + f[1]) / 2.0).collect();
    let coarse: Vec<f32> = mono
        .chunks_exact(GLIDE_DECIMATION)
        .map(|c| (((c[0] + c[1]) + c[2]) + c[3]) / GLIDE_DECIMATION as f32)
        .collect();

    let rising: Vec<f32> = (0..window).map(|n| (0.5 - 0.5 * (2.0 * PI * n as f64 / window as f64).cos()) as f32).collect();
    // Nothing comes before the first frame, so it does not fade in.
    let mut first = rising.clone();
    first[..hop].fill(1.0);

    let attacks = find_attacks(&mono);
    let mut out = vec![0.0f32; ((positions.len().max(1) - 1) * hop + window) * CHANNELS];
    let last = padded_frames - window;
    let mut start = 0;
    for (m, &position) in positions.iter().enumerate() {
        let nominal = ((position.round_ties_even() as i64 + pad as i64).max(0) as usize).min(last);
        let shape = if m == 0 {
            start = nominal;
            &first
        } else {
            // A slice of Python: it stops at the end.
            let from = (nominal / GLIDE_BLOCK).min(attacks.len());
            let to = ((nominal + window) / GLIDE_BLOCK + 1).min(attacks.len()).max(from);
            let tolerance = if attacks[from..to].iter().any(|&a| a) { GLIDE_ATTACK_TOLERANCE } else { GLIDE_TOLERANCE };
            start = best_start(&coarse, &mono, start + hop, nominal, tolerance).min(last);
            &rising
        };
        for (n, &w) in shape.iter().enumerate() {
            for c in 0..CHANNELS {
                out[(m * hop + n) * CHANNELS + c] += padded[(start + n) * CHANNELS + c] * w;
            }
        }
    }
    out.truncate(frames * CHANNELS);
    out
}

/// Make the pieces of a blend, the tempo of which glides between tracks. The tempo is `bpm` at the start (the
/// outgoing track's own) and `bpm / speed` at the end (the incoming track's own), growing by the same amount on
/// each beat. Both tracks follow it, so their beats stay together all the way and nothing is stretched once the
/// blend is over.
///
/// `outgoing` starts at the blend start in the outgoing track and holds `length` samples (a whole number of
/// beats, and `GLIDE_MARGIN` more if the track has them), `incoming` starts where the blend starts in the
/// incoming track and holds `length * speed`. Returns the two pieces, equally long, and where each of their
/// beats is.
pub fn tempo_glide(outgoing: &[f32], incoming: &[f32], bpm: f64, speed: f64, length: usize) -> (Vec<f32>, Vec<f32>, Vec<f64>) {
    let period = f64::from(SAMPLE_RATE) * 60.0 / bpm;
    let beats = length as f64 / period;
    if (speed - 1.0).abs() < NEGLIGIBLE_TEMPO_SHIFT {
        let frames = length.min(outgoing.len() / CHANNELS).min(incoming.len() / CHANNELS);
        let grid = (0..=beats as usize).map(|i| i as f64 * period).collect();
        return (outgoing[..frames * CHANNELS].to_vec(), incoming[..frames * CHANNELS].to_vec(), grid);
    }

    let target = bpm / speed;
    // The tempo grows by `step` bpm a beat, which makes it bpm * exp(rate * t) in time (t in samples).
    let step = (target - bpm) / beats;
    let rate = step / (60.0 * f64::from(SAMPLE_RATE));
    let frames = ((target / bpm).ln() / rate) as usize;
    // Outgoing samples read by each time.
    let pace: Vec<f64> =
        (0..(frames as f64 / GLIDE_HOP as f64).ceil() as usize + 2).map(|i| ((rate * (i * GLIDE_HOP) as f64).exp() - 1.0) / rate).collect();
    let grid = (0..=beats as usize).map(|i| ((bpm + step * i as f64) / bpm).ln() / rate).collect();
    let faster: Vec<f64> = pace.iter().map(|p| p * speed).collect();
    (wsola(outgoing, &pace, frames), wsola(incoming, &faster, frames), grid)
}
