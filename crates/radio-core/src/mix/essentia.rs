//! The key and the loudness by Essentia (feature `essentia`), task 1.5 of docs/rust-port.md.
//! Ported from `dj_mix.py`: `essentia_key`, and the Essentia branch of `measure_loudness`.
//! The C++ is in native/shim.cpp, built by build.rs from vendor/essentia.

use super::{CHANNELS, SAMPLE_RATE};
use std::ffi::{c_char, c_int};

unsafe extern "C" {
    fn ew_key(mono: *const f32, frames: usize, sample_rate: c_int, name: *mut c_char, minor: *mut c_int, strength: *mut f32) -> c_int;
    fn ew_loudness(stereo: *const f32, frames: usize, sample_rate: c_int, integrated: *mut f64) -> c_int;
}

const NOTES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// The tonic as Essentia spells it (flats too) to 0-11.
fn tonic(name: &str) -> Option<i32> {
    let flat = |sharp: &str| NOTES.iter().position(|&n| n == sharp).map(|i| i as i32);
    match name {
        "Db" => flat("C#"),
        "Eb" => flat("D#"),
        "Gb" => flat("F#"),
        "Ab" => flat("G#"),
        "Bb" => flat("A#"),
        _ => flat(name),
    }
}

/// Key by Essentia, with the profile made for electronic music, numbered as `TrackAnalysis::key`. -1 when Essentia
/// has no answer: it gave no key, or failed.
pub fn essentia_key(mono: &[f32]) -> i32 {
    let mut name = [0 as c_char; 4];
    let (mut minor, mut strength) = (0 as c_int, 0f32);
    // SAFETY: the pointers are to live buffers of the sizes shim.cpp is told or documents (name: 4 bytes).
    let failed = unsafe { ew_key(mono.as_ptr(), mono.len(), SAMPLE_RATE as c_int, name.as_mut_ptr(), &mut minor, &mut strength) };
    if failed != 0 || strength == 0.0 {
        return -1;
    }
    let bytes: Vec<u8> = name.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    let Some(note) = std::str::from_utf8(&bytes).ok().and_then(tonic) else { return -1 };
    note + if minor != 0 { 12 } else { 0 }
}

/// The integrated loudness (LUFS) of a stereo, interleaved piece by Essentia, `None` when it fails.
pub fn essentia_loudness(stereo: &[f32]) -> Option<f64> {
    let mut integrated = 0f64;
    // SAFETY: `stereo` holds whole frames of CHANNELS samples.
    let failed = unsafe { ew_loudness(stereo.as_ptr(), stereo.len() / CHANNELS, SAMPLE_RATE as c_int, &mut integrated) };
    (failed == 0).then_some(integrated)
}
