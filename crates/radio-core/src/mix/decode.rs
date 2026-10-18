//! Decoding a file to float32 44.1 kHz stereo, task 1.2 of docs/rust-port.md: `ffmpeg` does it, as in
//! `decode_file` of `dj.py`. Stage 8.1: `symphonia` does it first ([`decode_native`]); `ffmpeg` is the way out for what it
//! can't read (HE-AAC, Opus, more than two channels).

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
    match decode_native(path) {
        Ok(samples) => return Ok(samples),
        Err(error) => tracing::debug!("{} is not decoded natively ({error}), trying ffmpeg", path.display()),
    }
    decode_with_ffmpeg(ffmpeg, path)
}

/// The same, with `ffmpeg` only.
pub fn decode_with_ffmpeg(ffmpeg: &Path, path: &Path) -> io::Result<Vec<f32>> {
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

/// The same as [`decode_file_with`] for a task that may be stopped: ffmpeg is killed when the future is dropped.
pub async fn decode_file_async(ffmpeg: &Path, path: &Path) -> io::Result<Vec<f32>> {
    let native = {
        let path = path.to_owned();
        tokio::task::spawn_blocking(move || decode_native(&path)).await.map_err(io::Error::other)?
    };
    match native {
        Ok(samples) => return Ok(samples),
        Err(error) => tracing::debug!("{} is not decoded natively ({error}), trying ffmpeg", path.display()),
    }
    let output = tokio::process::Command::new(ffmpeg)
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-f", "f32le", "-ac", &CHANNELS.to_string(), "-ar", &SAMPLE_RATE.to_string(), "pipe:1"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    // The tail of a cut frame is left out.
    let usable = output.stdout.len() / (4 * CHANNELS) * (4 * CHANNELS);
    if !output.status.success() || usable == 0 {
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        return Err(io::Error::other(format!("ffmpeg could not decode {name}")));
    }
    Ok(output.stdout[..usable].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
}

/// Why the native decoder gave nothing: the file may still be read by ffmpeg.
#[derive(Debug)]
pub enum NativeError {
    /// A codec, a container or a channel layout `symphonia` can't do.
    Unsupported(String),
    /// The file is damaged or not there.
    Failed(String),
}

impl std::fmt::Display for NativeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(why) | Self::Failed(why) => f.write_str(why),
        }
    }
}

/// Decode a file to stereo, interleaved float32 at 44.1 kHz with `symphonia` (and `rubato` for another sample rate).
pub fn decode_native(path: &Path) -> Result<Vec<f32>, NativeError> {
    use symphonia::core::{
        codecs::audio::AudioDecoderOptions,
        errors::Error,
        formats::{FormatOptions, TrackType, probe::Hint},
        io::MediaSourceStream,
        meta::MetadataOptions,
    };
    let unsupported = |error: Error| match error {
        Error::Unsupported(why) => NativeError::Unsupported(why.to_string()),
        other => NativeError::Failed(other.to_string()),
    };
    let file = std::fs::File::open(path).map_err(|e| NativeError::Failed(e.to_string()))?;
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, MediaSourceStream::new(Box::new(file), Default::default()), FormatOptions::default(), MetadataOptions::default())
        .map_err(unsupported)?;
    let track = format.default_track(TrackType::Audio).ok_or_else(|| NativeError::Unsupported("no audio track".into()))?;
    let track_id = track.id;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or_else(|| NativeError::Unsupported("no audio parameters".into()))?;
    let mut decoder = symphonia::default::get_codecs().make_audio_decoder(params, &AudioDecoderOptions::default()).map_err(unsupported)?;

    let (mut channels, mut rate) = (0usize, 0u32);
    let mut samples: Vec<f32> = Vec::new();
    let mut chunk: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            // The end of a stream that has no mark of one.
            Err(Error::IoError(error)) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(unsupported(error)),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let spec = decoded.spec();
                let (count, hz) = (spec.channels().count(), spec.rate());
                if channels == 0 {
                    (channels, rate) = (count, hz);
                } else if (count, hz) != (channels, rate) {
                    return Err(NativeError::Unsupported("the format changes in the middle".into()));
                }
                chunk.clear();
                decoded.copy_to_vec_interleaved(&mut chunk);
                samples.extend_from_slice(&chunk);
            }
            // A packet that can't be decoded is skipped.
            Err(Error::IoError(_) | Error::DecodeError(_)) => {}
            Err(error) => return Err(unsupported(error)),
        }
    }
    if channels == 0 || samples.is_empty() {
        return Err(NativeError::Failed("nothing decoded".into()));
    }
    let stereo: Vec<f32> = match channels {
        // (what ffmpeg makes of a mono file: the centre goes to both sides at -3 dB)
        1 => samples.iter().flat_map(|&s| [s * std::f32::consts::FRAC_1_SQRT_2; 2]).collect(),
        2 => samples,
        n => return Err(NativeError::Unsupported(format!("{n} channels"))),
    };
    if rate == SAMPLE_RATE as u32 {
        return Ok(stereo);
    }
    resample(&stereo, rate as usize)
}

fn resample(stereo: &[f32], from: usize) -> Result<Vec<f32>, NativeError> {
    use audioadapter_buffers::direct::InterleavedSlice;
    use rubato::{Fft, FixedSync, Resampler};
    let frames = stereo.len() / CHANNELS;
    let failed = |e: &dyn std::fmt::Display| NativeError::Failed(format!("resampling: {e}"));
    let mut resampler = Fft::<f32>::new(from, SAMPLE_RATE as usize, 1024, CHANNELS, FixedSync::Both).map_err(|e| failed(&e))?;
    let input = InterleavedSlice::new(stereo, CHANNELS, frames).map_err(|e| failed(&e))?;
    let output = resampler.process_all(&input, frames, None).map_err(|e| failed(&e))?;
    Ok(output.take_data())
}
