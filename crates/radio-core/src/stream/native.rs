//! The encoders built into the program (stage 8.2 of docs/rust-port.md): MP3 by LAME (`mp3lame-encoder`), FLAC by `flacenc`
//! with the triangular dither of ffmpeg's `aresample`, and FLAC in Ogg pages made here. They stand in for the `ffmpeg`
//! process of [`super::encoder::FfmpegEncoder`] behind the same [`Encoder`] trait.

use super::broadcaster::Broadcaster;
use super::encoder::{Encoder, Format};
use super::frames::ogg_crc;
use crate::mix::{CHANNELS, SAMPLE_RATE};
use bytes::Bytes;
use flacenc::{
    bitsink::ByteSink,
    component::{BitRepr, StreamInfo},
    config,
    error::{Verified, Verify},
    source::{FrameBuf, MemSource, Source},
};
use mp3lame_encoder::{Bitrate, Builder, FlushNoGap, InterleavedPcm, Quality};
use std::{io, sync::Arc};

/// Samples in a FLAC frame.
const BLOCK: usize = 4096;
/// The longest an Ogg page is allowed to grow before it is written, in samples (100 ms, as `-page_duration 100000`).
const PAGE_SAMPLES: u64 = 4410;
const MP3_KBPS: Bitrate = Bitrate::Kbps320;

/// A source of uniform numbers in [0, 1) for the dither; it needs no quality, only to be uncorrelated with the music.
struct Dither(u64);

impl Dither {
    fn new() -> Self {
        Self(rand::random::<u64>() | 1)
    }

    fn uniform(&mut self) -> f32 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 40) as f32) / (1u64 << 24) as f32
    }

    /// Float samples to 16 bit with a triangular dither of one step (peak ±1) before the rounding.
    fn to_i16(&mut self, pcm: &[f32], out: &mut Vec<i32>) {
        for &sample in pcm {
            let noise = self.uniform() - self.uniform();
            let scaled = sample * 32768.0 + noise;
            out.push(scaled.round().clamp(-32768.0, 32767.0) as i32);
        }
    }
}

fn flac_stream_header() -> Vec<u8> {
    // "fLaC", then STREAMINFO as the last block: the sizes of blocks and frames, the rate, the channels, the depth, the
    // number of samples (not known) and the MD5 (not made).
    let mut info = Vec::with_capacity(34);
    info.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    info.extend_from_slice(&(BLOCK as u16).to_be_bytes());
    info.extend_from_slice(&[0; 6]); // frame sizes: not known
    let packed: u64 =
        (u64::from(SAMPLE_RATE as u32) << 44) | (((CHANNELS - 1) as u64) << 41) | (15 << 36);
    info.extend_from_slice(&packed.to_be_bytes());
    info.extend_from_slice(&[0; 16]);
    let mut out = b"fLaC".to_vec();
    out.push(0x80); // the last block, type STREAMINFO
    out.extend_from_slice(&[0, 0, info.len() as u8]);
    out.extend_from_slice(&info);
    out
}

enum Kind {
    Mp3(mp3lame_encoder::Encoder),
    Flac {
        frames: u64,
    },
    Ogg {
        frames: u64,
        serial: u32,
        sequence: u32,
        samples: u64,
        page: Vec<Vec<u8>>,
        page_samples: u64,
    },
}

/// An encoder inside the program.
pub struct NativeEncoder {
    hub: Arc<Broadcaster>,
    kind: Kind,
    dither: Dither,
    /// Interleaved samples waiting for a whole block.
    pending: Vec<f32>,
    flac: Option<(Verified<config::Encoder>, StreamInfo)>,
}

fn ogg_page(serial: u32, sequence: u32, granule: i64, first: bool, packets: &[Vec<u8>]) -> Vec<u8> {
    let mut lacing: Vec<u8> = Vec::new();
    for packet in packets {
        let mut left = packet.len();
        while left >= 255 {
            lacing.push(255);
            left -= 255;
        }
        lacing.push(left as u8);
    }
    let mut page = b"OggS".to_vec();
    page.push(0);
    page.push(if first { 0x02 } else { 0 });
    page.extend_from_slice(&granule.to_le_bytes());
    page.extend_from_slice(&serial.to_le_bytes());
    page.extend_from_slice(&sequence.to_le_bytes());
    page.extend_from_slice(&[0; 4]);
    page.push(lacing.len() as u8);
    page.extend_from_slice(&lacing);
    for packet in packets {
        page.extend_from_slice(packet);
    }
    let crc = ogg_crc(&page);
    page[22..26].copy_from_slice(&crc.to_le_bytes());
    page
}

impl NativeEncoder {
    pub fn start(format: Format, hub: Arc<Broadcaster>) -> io::Result<Self> {
        let flac = |_: ()| -> io::Result<(Verified<config::Encoder>, StreamInfo)> {
            let config = config::Encoder::default()
                .into_verified()
                .map_err(|(_, e)| io::Error::other(format!("{e:?}")))?;
            let info = StreamInfo::new(SAMPLE_RATE as usize, CHANNELS, 16)
                .map_err(|e| io::Error::other(format!("{e:?}")))?;
            Ok((config, info))
        };
        let (kind, flac) = match format {
            Format::Mp3 => {
                let build =
                    |e: mp3lame_encoder::BuildError| io::Error::other(format!("lame: {e:?}"));
                let mut builder =
                    Builder::new().ok_or_else(|| io::Error::other("lame could not start"))?;
                builder.set_num_channels(CHANNELS as u8).map_err(build)?;
                builder.set_sample_rate(SAMPLE_RATE as u32).map_err(build)?;
                builder.set_brate(MP3_KBPS).map_err(build)?;
                builder.set_quality(Quality::Good).map_err(build)?;
                (Kind::Mp3(builder.build().map_err(build)?), None)
            }
            Format::Flac => {
                let header = flac_stream_header();
                hub.set_header(header);
                (Kind::Flac { frames: 0 }, Some(flac(())?))
            }
            Format::Ogg => {
                let serial: u32 = rand::random();
                // The first packet: the mapping's mark, the version, the number of header packets after it, "fLaC" and
                // STREAMINFO; then the comments.
                let streaminfo = &flac_stream_header()[4..];
                let mut id = vec![0x7F];
                id.extend_from_slice(b"FLAC");
                id.extend_from_slice(&[1, 0, 0, 1]);
                id.extend_from_slice(b"fLaC");
                let mut block = streaminfo.to_vec();
                block[0] = 0; // not the last block: the comments follow
                id.extend_from_slice(&block);
                let vendor = b"radio-core";
                let mut comments = vec![0x84];
                let body_length = 4 + vendor.len() + 4;
                comments.extend_from_slice(&(body_length as u32).to_be_bytes()[1..]);
                comments.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
                comments.extend_from_slice(vendor);
                comments.extend_from_slice(&0u32.to_le_bytes());
                let mut header = ogg_page(serial, 0, 0, true, &[id]);
                header.extend_from_slice(&ogg_page(serial, 1, 0, false, &[comments]));
                hub.set_header(header);
                (
                    Kind::Ogg {
                        frames: 0,
                        serial,
                        sequence: 2,
                        samples: 0,
                        page: Vec::new(),
                        page_samples: 0,
                    },
                    Some(flac(())?),
                )
            }
        };
        Ok(Self {
            hub,
            kind,
            dither: Dither::new(),
            pending: Vec::new(),
            flac,
        })
    }

    fn flac_frame(&mut self, block: &[f32]) -> io::Result<Vec<u8>> {
        let mut ints = Vec::with_capacity(block.len());
        self.dither.to_i16(block, &mut ints);
        let (config, info) = self.flac.as_ref().expect("a FLAC kind has the encoder");
        let frames = match &mut self.kind {
            Kind::Flac { frames } | Kind::Ogg { frames, .. } => frames,
            Kind::Mp3(_) => unreachable!("an MP3 has no FLAC frames"),
        };
        let mut buffer = FrameBuf::with_size(CHANNELS, block.len() / CHANNELS)
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        let mut source = MemSource::from_samples(&ints, CHANNELS, 16, SAMPLE_RATE as usize);
        source
            .read_samples(block.len() / CHANNELS, &mut buffer)
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        let frame = flacenc::encode_fixed_size_frame(config, &buffer, *frames as usize, info)
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        *frames += 1;
        let mut sink = ByteSink::new();
        frame
            .write(&mut sink)
            .map_err(|e| io::Error::other(format!("{e:?}")))?;
        Ok(sink.as_slice().to_vec())
    }

    /// Encode one block of whole frames of the interleaved samples.
    fn encode_block(&mut self, block: &[f32]) -> io::Result<()> {
        if let Kind::Mp3(encoder) = &mut self.kind {
            let mut out = Vec::with_capacity(mp3lame_encoder::max_required_buffer_size(
                block.len() / CHANNELS,
            ));
            encoder
                .encode_to_vec(InterleavedPcm(block), &mut out)
                .map_err(|e| io::Error::other(format!("lame: {e:?}")))?;
            if !out.is_empty() {
                self.hub.publish(Bytes::from(out));
            }
            return Ok(());
        }
        let packet = self.flac_frame(block)?;
        let in_block = (block.len() / CHANNELS) as u64;
        match &mut self.kind {
            Kind::Flac { .. } => self.hub.publish(Bytes::from(packet)),
            Kind::Ogg {
                serial,
                sequence,
                samples,
                page,
                page_samples,
                ..
            } => {
                *samples += in_block;
                *page_samples += in_block;
                page.push(packet);
                // (a page is written when another block would take it over 100 ms: with 93 ms blocks, one for every page)
                if *page_samples + BLOCK as u64 > PAGE_SAMPLES
                    || page.iter().map(Vec::len).sum::<usize>() > 60_000
                {
                    let bytes = ogg_page(*serial, *sequence, *samples as i64, false, page);
                    *sequence += 1;
                    page.clear();
                    *page_samples = 0;
                    self.hub.publish(Bytes::from(bytes));
                }
            }
            Kind::Mp3(_) => unreachable!(),
        }
        Ok(())
    }

    /// The mix is over: the samples that wait are encoded too.
    pub fn finish(mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.encode_block(&rest)?;
        }
        if let Kind::Mp3(encoder) = &mut self.kind {
            let mut out = Vec::with_capacity(7200);
            encoder
                .flush_to_vec::<FlushNoGap>(&mut out)
                .map_err(|e| io::Error::other(format!("lame: {e:?}")))?;
            if !out.is_empty() {
                self.hub.publish(Bytes::from(out));
            }
        }
        Ok(())
    }
}

impl Encoder for NativeEncoder {
    async fn write(&mut self, pcm: &[f32]) -> io::Result<()> {
        // LAME takes any amount; FLAC wants whole blocks.
        if matches!(self.kind, Kind::Mp3(_)) {
            return self.encode_block(pcm);
        }
        self.pending.extend_from_slice(pcm);
        let block = BLOCK * CHANNELS;
        let mut used = 0;
        while self.pending.len() - used >= block {
            let chunk = self.pending[used..used + block].to_vec();
            self.encode_block(&chunk)?;
            used += block;
        }
        self.pending.drain(..used);
        Ok(())
    }

    async fn stop(self) {}
}

/// Either of the encoders, for a program that picks one when it starts.
pub enum AnyEncoder {
    Native(NativeEncoder),
    Ffmpeg(super::encoder::FfmpegEncoder),
}

impl Encoder for AnyEncoder {
    async fn write(&mut self, pcm: &[f32]) -> io::Result<()> {
        match self {
            Self::Native(encoder) => encoder.write(pcm).await,
            Self::Ffmpeg(encoder) => encoder.write(pcm).await,
        }
    }

    async fn stop(self) {
        match self {
            Self::Native(encoder) => encoder.stop().await,
            Self::Ffmpeg(encoder) => encoder.stop().await,
        }
    }
}
