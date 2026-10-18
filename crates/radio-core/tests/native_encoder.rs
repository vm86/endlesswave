//! The encoders built into the program (stage 8.2): what they write is decoded by symphonia and by ffmpeg (if there is one)
//! and compared with what went in. The streams must also be ones the Broadcaster can cut: header lengths, frame and page
//! starts.

use radio_core::stream::{
    broadcaster::Broadcaster,
    encoder::{Encoder, Format},
    frames::{OggPage, flac_header_length, flac_start, ogg_header_length, ogg_page_length, ogg_start},
    native::NativeEncoder,
};
use std::{fs, path::PathBuf};

const SR: usize = 44100;

/// A few seconds of something music-like: two chords with a click track, different on the two sides.
fn signal(seconds: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(seconds * SR * 2);
    let mut noise = 12345u32;
    for n in 0..seconds * SR {
        let t = n as f32 / SR as f32;
        noise = noise.wrapping_mul(1664525).wrapping_add(1013904223);
        let hiss = ((noise >> 9) as f32 / (1 << 23) as f32 - 0.5) * 0.02;
        let beat = (-(t * 2.0).fract() * 30.0).exp() * (2.0 * std::f32::consts::PI * 60.0 * t).sin() * 0.4;
        let chord = [220.0f32, 277.2, 329.6].iter().map(|f| (2.0 * std::f32::consts::PI * f * t).sin() * 0.15).sum::<f32>();
        out.push(chord + beat + hiss);
        out.push(chord * 0.8 - beat + hiss);
    }
    out
}

async fn encode(format: Format, pcm: &[f32]) -> (Vec<u8>, std::sync::Arc<Broadcaster>) {
    let hub = Broadcaster::new(format.config());
    let listener = hub.subscribe();
    let mut encoder = NativeEncoder::start(format, hub.clone()).unwrap();
    // pieces of odd sizes, as the mix gives them
    for piece in pcm.chunks(2 * 7777) {
        encoder.write(piece).await.unwrap();
    }
    encoder.finish().unwrap();
    let mut data = Vec::new();
    while let Some(chunk) = listener.try_recv() {
        data.extend_from_slice(&chunk);
    }
    // (the listener was there before the header was known, and has it first)
    assert!(hub.header().is_empty() || data.starts_with(&hub.header()), "the header comes first");
    (data, hub)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("native-encoder-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Decoded by symphonia, as the radio would read it.
fn decoded(bytes: &[u8], name: &str) -> Vec<f32> {
    let path = scratch(name);
    fs::write(&path, bytes).unwrap();
    radio_core::mix::decode::decode_native(&path).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The SNR of `got` against `want`, with the best shift of up to `max_shift` frames.
fn snr_with_shift(got: &[f32], want: &[f32], max_shift: i64) -> (f64, i64) {
    let mut best = (f64::MIN, 0);
    for shift in -max_shift..=max_shift {
        let (mut signal, mut noise) = (0f64, 0f64);
        for i in (20_000..want.len().min(got.len()) - 20_000).step_by(2) {
            let j = (i as i64 + shift * 2) as usize;
            let (w, d) = (f64::from(want[i]), f64::from(got[j]) - f64::from(want[i]));
            signal += w * w;
            noise += d * d;
        }
        let snr = 10.0 * (signal / noise.max(1e-30)).log10();
        if snr > best.0 {
            best = (snr, shift);
        }
    }
    best
}

#[tokio::test]
async fn the_flac_stream_is_lossless_to_sixteen_bits_and_can_be_cut() {
    let pcm = signal(12);
    let (bytes, hub) = encode(Format::Flac, &pcm).await;
    assert!(bytes.starts_with(b"fLaC"));
    let header = flac_header_length(&bytes).unwrap().expect("the header is whole");
    assert_eq!(hub.header().len(), header, "the broadcaster keeps just the header");
    assert_eq!(flac_start(&bytes[header..]), 0, "a frame starts where the header ends");
    let back = decoded(&bytes, "a.flac");
    assert_eq!(back.len(), pcm.len(), "no samples lost");
    let (snr, shift) = snr_with_shift(&back, &pcm, 0);
    assert!(snr > 80.0, "{snr:.1} dB at shift {shift}");
    if let Some(ffmpeg) = ffmpeg_decode(&bytes, "a2.flac") {
        assert_eq!(ffmpeg.len(), pcm.len());
        assert!(snr_with_shift(&ffmpeg, &pcm, 0).0 > 80.0, "ffmpeg reads the stream too");
    }
}

#[tokio::test]
async fn the_ogg_stream_holds_the_same_flac_in_checked_pages() {
    let pcm = signal(12);
    let (bytes, hub) = encode(Format::Ogg, &pcm).await;
    // every page is whole and its CRC is right
    let mut place = 0;
    let mut pages = 0;
    while place < bytes.len() {
        match ogg_page_length(&bytes, place) {
            OggPage::Complete(size) => place += size,
            other => panic!("page {pages} at {place}: {other:?}"),
        }
        pages += 1;
    }
    assert!(pages > 100, "{pages} pages for 12 s: about one in 100 ms");
    let header = ogg_header_length(&bytes).unwrap().expect("the header is whole");
    assert_eq!(hub.header().len(), header);
    assert_eq!(ogg_start(&bytes[header..]), 0);
    let back = decoded(&bytes, "a.ogg");
    assert_eq!(back.len(), pcm.len());
    assert!(snr_with_shift(&back, &pcm, 0).0 > 80.0);
    if let Some(ffmpeg) = ffmpeg_decode(&bytes, "a2.ogg") {
        assert_eq!(ffmpeg.len(), pcm.len());
        assert!(snr_with_shift(&ffmpeg, &pcm, 0).0 > 80.0, "ffmpeg reads the pages too");
    }
}

#[tokio::test]
async fn the_mp3_stream_is_the_music() {
    let pcm = signal(12);
    let (bytes, _) = encode(Format::Mp3, &pcm).await;
    assert!(bytes.len() > 12 * 40_000 - 20_000, "320 kbit/s is 40 kB a second, there are {}", bytes.len());
    assert!(bytes.len() < 12 * 40_000 + 20_000);
    let back = decoded(&bytes, "a.mp3");
    // a lossy codec with a delay: the best shift tells how much
    let (snr, shift) = snr_with_shift(&back, &pcm, 2400);
    assert!(snr > 25.0, "{snr:.1} dB at shift {shift}");
    println!("mp3: {snr:.1} dB at shift {shift} frames");
}

fn ffmpeg_decode(bytes: &[u8], name: &str) -> Option<Vec<f32>> {
    let path = scratch(name);
    fs::write(&path, bytes).unwrap();
    radio_core::mix::decode::decode_with_ffmpeg(std::path::Path::new("ffmpeg"), &path).ok()
}
