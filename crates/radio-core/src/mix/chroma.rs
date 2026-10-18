//! The key without Essentia: the pitch classes of the track against the Krumhansl-Kessler profiles. Task 1.5.4 of
//! docs/rust-port.md, ported from `dj_mix.py`: `chroma`, `profile_key`, and the fall back of `estimate_key`.
//! Used where Essentia is not built in, and where it has no answer.

use super::SAMPLE_RATE;
use realfft::RealFftPlanner;
use std::f64::consts::PI;

const KEY_FRAME: usize = 8192;
const KEY_HOP: usize = 16384;
const KEY_MIN_HZ: f64 = 110.0;
const KEY_MAX_HZ: f64 = 2000.0;
/// Frames quieter than this share of the loudest one are not looked at.
const KEY_QUIET_RATIO: f64 = 0.1;
/// Below this correlation the track has no tonal centre worth a label.
const MIN_KEY_CONFIDENCE: f64 = 0.3;
const PITCH_CLASSES: usize = 12;
const MAJOR_PROFILE: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR_PROFILE: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// Energy of the track at each of the 12 pitch classes (C = 0).
pub fn chroma(mono: &[f32]) -> [f64; 12] {
    let usable: Vec<(usize, usize)> = (0..=KEY_FRAME / 2)
        .filter_map(|bin| {
            let freq = bin as f64 * f64::from(SAMPLE_RATE) / KEY_FRAME as f64;
            (KEY_MIN_HZ..=KEY_MAX_HZ).contains(&freq).then(|| {
                let class = (12.0 * (freq / 440.0).log2() + 9.0).round_ties_even() as i64;
                (bin, class.rem_euclid(PITCH_CLASSES as i64) as usize)
            })
        })
        .collect();
    let starts: Vec<usize> = (0..)
        .map(|i| i * KEY_HOP)
        .take_while(|&s| s + KEY_FRAME < mono.len())
        .collect();
    if starts.is_empty() {
        return [0.0; 12];
    }
    // np.hanning: symmetric.
    let window: Vec<f32> = (0..KEY_FRAME)
        .map(|n| (0.5 - 0.5 * (2.0 * PI * n as f64 / (KEY_FRAME - 1) as f64).cos()) as f32)
        .collect();
    let fft = RealFftPlanner::<f32>::new().plan_fft_forward(KEY_FRAME);
    let mut input = fft.make_input_vec();
    let mut output = fft.make_output_vec();
    let mut rows: Vec<[f64; 12]> = Vec::with_capacity(starts.len());
    for &start in &starts {
        for ((x, &s), &w) in input
            .iter_mut()
            .zip(&mono[start..start + KEY_FRAME])
            .zip(&window)
        {
            *x = s * w;
        }
        fft.process(&mut input, &mut output)
            .expect("sizes are right");
        let mut row = [0.0f64; 12];
        // The square root keeps a loud bass note from drowning the harmony.
        for &(bin, class) in &usable {
            row[class] += f64::from(output[bin].norm().sqrt());
        }
        rows.push(row);
    }
    let totals: Vec<f64> = rows.iter().map(|row| row.iter().sum()).collect();
    let loudest = totals.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mut total = [0.0f64; 12];
    for (row, &sum) in rows.iter().zip(&totals) {
        if sum > KEY_QUIET_RATIO * loudest {
            // Every frame counts the same, whatever its loudness.
            for (t, &r) in total.iter_mut().zip(row) {
                *t += r / sum;
            }
        }
    }
    total
}

/// The Pearson correlation of two series.
fn correlation(a: &[f64; 12], b: &[f64; 12]) -> f64 {
    let (mean_a, mean_b) = (a.iter().sum::<f64>() / 12.0, b.iter().sum::<f64>() / 12.0);
    let (mut covariance, mut var_a, mut var_b) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        covariance += (x - mean_a) * (y - mean_b);
        var_a += (x - mean_a).powi(2);
        var_b += (y - mean_b).powi(2);
    }
    covariance / (var_a * var_b).sqrt()
}

/// The key whose profile fits the chroma best (numbered as `TrackAnalysis::key`), and how well.
pub fn profile_key(mono: &[f32]) -> (i32, f64) {
    let profile = chroma(mono);
    if profile.iter().all(|&p| p == 0.0) {
        return (-1, 0.0);
    }
    let (mut best, mut best_score) = (-1, 0.0);
    for (offset, reference) in [(0, &MAJOR_PROFILE), (PITCH_CLASSES, &MINOR_PROFILE)] {
        for tonic in 0..PITCH_CLASSES {
            // np.roll(reference, tonic)
            let rolled: [f64; 12] =
                std::array::from_fn(|i| reference[(i + PITCH_CLASSES - tonic) % PITCH_CLASSES]);
            let score = correlation(&profile, &rolled);
            if score > best_score {
                (best, best_score) = ((offset + tonic) as i32, score);
            }
        }
    }
    (best, best_score)
}

/// The key by the profiles, -1 if the track has no tonal centre.
pub fn profile_estimate(mono: &[f32]) -> i32 {
    let (key, confidence) = profile_key(mono);
    if confidence >= MIN_KEY_CONFIDENCE {
        key
    } else {
        -1
    }
}
