//! The audio core: analysis of a track, planning and making a blend, the choice of the next track.
//! Ported from `yamusic_radio/dj_mix.py`; the numbers are the same, so the tests can compare them.

use serde::{Deserialize, Serialize};

pub mod analysis;
pub mod blend;
pub mod choose;
pub mod chroma;
pub mod decode;
pub mod effects;
#[cfg(feature = "essentia")]
pub mod essentia;
pub mod glide;
pub mod key;
pub mod loudness;
pub mod plan;

pub const SAMPLE_RATE: u32 = 44100;
pub const CHANNELS: usize = 2;

/// Stereo samples, interleaved: `[left, right, left, right, ...]`.
pub type Pcm = Vec<f32>;

/// What the analysis finds in a track (`TrackAnalysis` of `dj_mix.py`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackAnalysis {
    /// 0 when the tempo could not be told.
    pub bpm: f64,
    /// Samples: a beat near the start of the track.
    pub head_beat: f64,
    /// Samples: a beat near the end of the track.
    pub tail_beat: f64,
    /// The first audible sample.
    pub start: i64,
    /// One past the last audible sample.
    pub end: i64,
    /// LUFS (EBU R128).
    pub loudness_db: f64,
    /// Where the quiet intro ends, -1 when it could not be told.
    pub intro_end: i64,
    /// Where the quiet outro begins, -1 when it could not be told.
    pub outro_start: i64,
    /// 0-11 a major key (C, C#, ... B), 12-23 the minor key with tonic 0-11, -1 when it could not be told.
    pub key: i32,
}

/// Name of what finds the keys: "essentia" or "chroma profiles" (what the cache of the Python radio calls them).
pub fn key_detector() -> &'static str {
    if cfg!(feature = "essentia") {
        "essentia"
    } else {
        "chroma profiles"
    }
}

/// Name of what measures the loudness. The `ebur128` crate does EBU R128 too, so it is "ebu r128" either way; "rms"
/// is only for the Python radio without Essentia.
pub fn loudness_meter() -> &'static str {
    "ebu r128"
}

/// Key of the track (see `TrackAnalysis::key`), -1 if there is none. Essentia tells the key much better than the plain
/// profile match, also for dense, distorted music, whose pitch classes the plain match finds as flat as noise.
/// Where Essentia has no answer, or is not built in, the plain match gives the key, if it finds a tonal centre.
pub fn estimate_key(mono: &[f32]) -> i32 {
    #[cfg(feature = "essentia")]
    {
        let found = essentia::essentia_key(mono);
        if found >= 0 {
            return found;
        }
    }
    chroma::profile_estimate(mono)
}

/// Analyze a stereo, interleaved track (`analyze` of `dj_mix.py`).
///
/// The key is found by Essentia (feature `essentia`), and by the profiles where it has no answer or is not built in.
pub fn analyze(pcm: &[f32]) -> TrackAnalysis {
    // The loudness first: its meter makes a copy of the stereo signal, and the copy is gone before the mono one (half as big)
    // and the one Essentia makes of that are made. Freed memory is reused by what is allocated next; all together there are
    // never more than two copies of the track.
    let (start, end) = analysis::audible_range(pcm);
    let loudness_db = loudness::measure_loudness(&pcm[start * CHANNELS..end * CHANNELS]);
    let (rhythm, mono) = analysis::rhythm_and_mono(pcm);
    let key = estimate_key(&mono);
    TrackAnalysis {
        bpm: rhythm.bpm,
        head_beat: rhythm.head_beat,
        tail_beat: rhythm.tail_beat,
        start: rhythm.start as i64,
        end: rhythm.end as i64,
        loudness_db,
        intro_end: rhythm.intro_end,
        outro_start: rhythm.outro_start,
        key,
    }
}
