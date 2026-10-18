//! Encoding the mix: the raw PCM goes in, the MP3, FLAC or Ogg stream comes out into a [`Broadcaster`]. Task 3.3 of
//! docs/rust-port.md, ported from `_start_encoder`, `_forward`, `_start_encoders` and `_stop_encoders` of `dj.py`.
//!
//! The work is done by an `ffmpeg` process; the [`Encoder`] trait is the seam to replace it with encoders built into
//! the program (stage 8).

use super::broadcaster::{Broadcaster, Config};
use crate::mix::{CHANNELS, SAMPLE_RATE};
use bytes::Bytes;
use std::{io, path::Path, process::Stdio, sync::Arc};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
};

/// The regular stream is HQ MP3; the lossless one is FLAC (16 bit, dithered), which Safari gets in Ogg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mp3,
    Flac,
    Ogg,
}

impl Format {
    /// The output arguments of ffmpeg.
    pub fn output_args(self) -> &'static [&'static str] {
        match self {
            Self::Mp3 => &["-c:a", "libmp3lame", "-b:a", "320k", "-f", "mp3"],
            // 16 bit with a triangular dither: what the CD does with a finer mix.
            Self::Flac => &[
                "-af",
                "aresample=dither_method=triangular",
                "-sample_fmt",
                "s16",
                "-c:a",
                "flac",
                "-f",
                "flac",
            ],
            // The same FLAC in Ogg pages, which are made as often as the muxer allows.
            Self::Ogg => &[
                "-af",
                "aresample=dither_method=triangular",
                "-sample_fmt",
                "s16",
                "-c:a",
                "flac",
                "-page_duration",
                "100000",
                "-f",
                "ogg",
            ],
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Mp3 => "audio/mpeg",
            Self::Flac => "audio/flac",
            Self::Ogg => "audio/ogg",
        }
    }

    /// How the Broadcaster of this stream cuts it.
    pub fn config(self) -> Config {
        match self {
            Self::Mp3 => Config::mp3(),
            Self::Flac => Config::flac(),
            Self::Ogg => Config::ogg(),
        }
    }
}

/// Takes the mix and makes a stream of it.
pub trait Encoder: Sized {
    /// Give the encoder more of the mix: stereo, interleaved, 44.1 kHz.
    fn write(&mut self, pcm: &[f32]) -> impl Future<Output = io::Result<()>> + Send;
    /// Stop at once.
    fn stop(self) -> impl Future<Output = ()> + Send;
}

/// An `ffmpeg` process: the mix in as raw float PCM, the encoded stream out.
pub struct FfmpegEncoder {
    child: Child,
    stdin: ChildStdin,
    forward: JoinHandle<()>,
}

impl FfmpegEncoder {
    /// Start ffmpeg and publish what it writes to `hub`.
    pub fn start(ffmpeg: &Path, format: Format, hub: Arc<Broadcaster>) -> io::Result<Self> {
        let mut child = Command::new(ffmpeg)
            .args([
                "-nostdin",
                "-v",
                "error",
                "-f",
                "f32le",
                "-ac",
                &CHANNELS.to_string(),
                "-ar",
                &SAMPLE_RATE.to_string(),
                "-i",
                "pipe:0",
            ])
            .args(format.output_args())
            .args(["-flush_packets", "1", "pipe:1"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err(io::Error::other("ffmpeg has no pipes"));
        };
        Ok(Self {
            child,
            stdin,
            forward: tokio::spawn(forward(stdout, hub)),
        })
    }

    /// No more of the mix: let ffmpeg write the rest and end. Waits until it has.
    pub async fn finish(mut self) -> io::Result<()> {
        self.stdin.shutdown().await?;
        drop(self.stdin);
        let _ = self.forward.await;
        self.child.wait().await?;
        Ok(())
    }
}

impl Encoder for FfmpegEncoder {
    async fn write(&mut self, pcm: &[f32]) -> io::Result<()> {
        let mut bytes = Vec::with_capacity(pcm.len() * 4);
        for sample in pcm {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.stdin.write_all(&bytes).await
    }

    async fn stop(mut self) {
        let _ = self.child.kill().await;
        self.forward.abort();
    }
}

/// Publish what an encoder writes; the header of a FLAC or Ogg stream is kept apart.
async fn forward(mut stdout: ChildStdout, hub: Arc<Broadcaster>) {
    let mut buffer = [0u8; 4096];
    let mut pending: Vec<u8> = Vec::new();
    if let Some(header_length) = hub.config().header_length {
        let length = loop {
            match header_length(&pending) {
                Ok(Some(length)) => break length,
                // not a stream of the kind: nothing to publish
                Err(_) => return,
                Ok(None) => {}
            }
            match stdout.read(&mut buffer).await {
                Ok(0) | Err(_) => return,
                Ok(n) => pending.extend_from_slice(&buffer[..n]),
            }
        };
        let rest = pending.split_off(length);
        hub.set_header(pending);
        if !rest.is_empty() {
            hub.publish(Bytes::from(rest));
        }
    }
    loop {
        match stdout.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(n) => hub.publish(Bytes::copy_from_slice(&buffer[..n])),
        }
    }
}
