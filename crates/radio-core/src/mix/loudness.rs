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

pub const TARGET_LUFS: f64 = -16.0;
const MAX_GAIN_DB: f64 = 12.0;
const MIN_NORMALIZE_LUFS: f64 = -60.0;

/// Apply one gain to a whole track, bounded by its sample peak. Returns the applied gain in dB.
/// Silence and unusable loudness measurements are not boosted.
pub fn normalize(pcm: &mut [f32], loudness_db: f64) -> f64 {
    let peak = pcm.iter().fold(0.0f32, |top, x| top.max(x.abs()));
    if peak == 0.0 {
        return 0.0;
    }
    let wanted_db = if loudness_db.is_finite() && loudness_db > MIN_NORMALIZE_LUFS {
        (TARGET_LUFS - loudness_db).min(MAX_GAIN_DB)
    } else {
        0.0
    };
    let gain = 10.0f64
        .powf(wanted_db / 20.0)
        .min(f64::from(super::effects::LIMIT_CEILING) / f64::from(peak));
    for sample in pcm {
        *sample = (f64::from(*sample) * gain) as f32;
    }
    20.0 * gain.log10()
}

/// The mean of the squares of the samples.
fn mean_square(pcm: &[f32]) -> f64 {
    if pcm.is_empty() {
        return 0.0;
    }
    pcm.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / pcm.len() as f64
}

/// Longest body (in samples, both channels) whose loudness is measured by Essentia: six minutes.
#[cfg(feature = "essentia")]
const ESSENTIA_LOUDNESS_MAX_SAMPLES: usize = 6 * 60 * 44100 * 2;

/// The integrated loudness of the audible part of a stereo, interleaved track in LUFS. For a piece shorter
/// than a gating block it is the RMS brought to the same scale.
pub fn measure_loudness(body: &[f32]) -> f64 {
    if body.len() as f64 >= MIN_LUFS_SECONDS * f64::from(SAMPLE_RATE) * CHANNELS as f64 {
        // Essentia, where it is built in, gives the very numbers of the Python radio.
        // (Essentia makes a copy of the signal, and works in more of its own; on a long track that is the biggest piece of
        // memory the radio ever uses, so there the streaming meter below, which differs by about 0.001 dB, is used.)
        #[cfg(feature = "essentia")]
        if body.len() <= ESSENTIA_LOUDNESS_MAX_SAMPLES
            && let Some(integrated) =
                super::essentia::essentia_loudness(body).filter(|l| l.is_finite())
        {
            return integrated;
        }
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
