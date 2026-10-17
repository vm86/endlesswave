//! The integrated loudness of a track in LUFS (EBU R128), task 1.4 of docs/rust-port.md.
//! The radio asks Essentia; here the `ebur128` crate measures it (ITU-R BS.1770: K-weighted, gated).

use super::{CHANNELS, SAMPLE_RATE};
use ebur128::{EbuR128, Mode};

/// EBU R128 gates in blocks of 400 ms: a shorter piece has no loudness.
const MIN_LUFS_SECONDS: f64 = 0.4;
/// What the RMS of the samples (all channels averaged) is short of the loudness in LUFS, for music.
const LUFS_OVER_RMS_DB: f64 = 2.0;
/// What Essentia says for a piece with no block above the absolute gate (silence), where libebur128 says -inf.
const NOTHING_LOUD_LUFS: f64 = -70.0;

/// The mean of the squares of the samples.
fn mean_square(pcm: &[f32]) -> f64 {
    if pcm.is_empty() {
        return 0.0;
    }
    pcm.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / pcm.len() as f64
}

/// The integrated loudness of the audible part of a stereo, interleaved track in LUFS. For a piece shorter
/// than a gating block it is the RMS brought to the same scale.
pub fn measure_loudness(body: &[f32]) -> f64 {
    if body.len() as f64 >= MIN_LUFS_SECONDS * f64::from(SAMPLE_RATE) * CHANNELS as f64 {
        let integrated = EbuR128::new(CHANNELS as u32, SAMPLE_RATE, Mode::I)
            .and_then(|mut meter| {
                meter.add_frames_f32(body)?;
                meter.loudness_global()
            })
            .unwrap_or(f64::NEG_INFINITY);
        if integrated.is_finite() {
            return integrated;
        }
        if integrated == f64::NEG_INFINITY {
            return NOTHING_LOUD_LUFS;
        }
    }
    20.0 * (mean_square(body) + 1e-12).sqrt().log10() + LUFS_OVER_RMS_DB
}
