//! Decoding a file to float32 44.1 kHz stereo, task 1.2 of docs/rust-port.md: `ffmpeg` does it, as in
//! `decode_file` of `dj.py`. (`symphonia` could one day, see stage 8.)

use super::{CHANNELS, SAMPLE_RATE};
use std::{
    io,
    path::Path,
    process::{Command, Stdio},
};

/// Decode a file to stereo, interleaved float32 at 44.1 kHz with the `ffmpeg` that is found in `PATH`.
pub fn decode_file(path: &Path) -> io::Result<Vec<f32>> {
    decode_file_with(Path::new("ffmpeg"), path)
}

/// The same with a given `ffmpeg`.
pub fn decode_file_with(ffmpeg: &Path, path: &Path) -> io::Result<Vec<f32>> {
    let output = Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-f", "f32le", "-ac", &CHANNELS.to_string(), "-ar", &SAMPLE_RATE.to_string(), "pipe:1"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    // The tail of a cut frame is left out.
    let usable = output.stdout.len() / (4 * CHANNELS) * (4 * CHANNELS);
    if !output.status.success() || usable == 0 {
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        return Err(io::Error::other(format!("ffmpeg could not decode {name}")));
    }
    Ok(output.stdout[..usable].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}
