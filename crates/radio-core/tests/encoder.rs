//! The encoders against the Python radio's: the same ffmpeg commands on the same mix give the same stream.
//! Task 3.3 of docs/rust-port.md. Needs `ffmpeg` with libmp3lame (the path may be given in `FFMPEG`) and the files of
//! `tools/gen_golden.py` (`tests/golden/stream/`, not in git); without them the tests are skipped, unless
//! `GOLDEN_REQUIRED` is set.

use radio_core::stream::{
    broadcaster::Broadcaster,
    encoder::{Encoder, FfmpegEncoder, Format},
    frames::{self, OggPage},
};
use std::{fs, path::PathBuf};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/stream")
}

fn ffmpeg() -> PathBuf {
    std::env::var_os("FFMPEG").map_or_else(|| PathBuf::from("ffmpeg"), PathBuf::from)
}

fn skip(why: &str) -> bool {
    assert!(std::env::var_os("GOLDEN_REQUIRED").is_none(), "{why}");
    eprintln!("{why}: skipped");
    true
}

fn pcm() -> Option<Vec<f32>> {
    let bytes = fs::read(dir().join("pcm.f32")).ok()?;
    Some(bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect())
}

fn has_ffmpeg() -> bool {
    std::process::Command::new(ffmpeg()).arg("-version").output().is_ok_and(|o| o.status.success())
}

/// Run the mix through an encoder, in pieces of a quarter of a second like the radio makes them, and give the whole
/// stream: the header, then what was published.
async fn encode(format: Format, pcm: &[f32]) -> Vec<u8> {
    let hub = Broadcaster::new(format.config());
    let listener = hub.subscribe();
    let mut encoder = FfmpegEncoder::start(&ffmpeg(), format, hub.clone()).expect("ffmpeg starts");
    for piece in pcm.chunks(44100 / 4 * 2) {
        encoder.write(piece).await.unwrap();
    }
    encoder.finish().await.unwrap();
    let mut stream = Vec::new();
    while let Some(chunk) = listener.try_recv() {
        stream.extend_from_slice(&chunk);
    }
    // (a listener who was there before the header was known has it first now)
    if stream.starts_with(&hub.header()) { stream } else { [hub.header(), stream].concat() }
}

#[tokio::test]
async fn mp3_and_flac_are_the_streams_python_makes() {
    let Some(pcm) = pcm() else { skip("no mix, run tools/gen_golden.py"); return };
    if !has_ffmpeg() {
        skip("no ffmpeg");
        return;
    }
    for (format, name) in [(Format::Mp3, "mp3"), (Format::Flac, "flac")] {
        let want = fs::read(dir().join(format!("{name}.bin"))).unwrap();
        let got = encode(format, &pcm).await;
        assert_eq!(got.len(), want.len(), "{name}: length");
        assert!(got == want, "{name}: the bytes differ");
    }
}

#[tokio::test]
async fn ogg_is_made_of_pages_as_python_makes_it() {
    let Some(pcm) = pcm() else { skip("no mix, run tools/gen_golden.py"); return };
    if !has_ffmpeg() {
        skip("no ffmpeg");
        return;
    }
    let want = fs::read(dir().join("ogg.bin")).unwrap();
    let got = encode(Format::Ogg, &pcm).await;
    // The serial number of the stream is random, so the bytes differ; the pages do not.
    let pages = |data: &[u8]| -> Vec<usize> {
        let mut place = 0;
        let mut sizes = Vec::new();
        while let OggPage::Complete(size) = frames::ogg_page_length(data, place) {
            sizes.push(size);
            place += size;
        }
        assert_eq!(place, data.len(), "the whole stream is pages");
        sizes
    };
    assert_eq!(pages(&got), pages(&want));
    assert_eq!(frames::ogg_header_length(&got), frames::ogg_header_length(&want));
}

#[tokio::test]
async fn a_listener_who_joins_gets_the_header_first_and_starts_on_a_frame() {
    let Some(pcm) = pcm() else { skip("no mix, run tools/gen_golden.py"); return };
    if !has_ffmpeg() {
        skip("no ffmpeg");
        return;
    }
    let hub = Broadcaster::new(Format::Flac.config());
    let mut encoder = FfmpegEncoder::start(&ffmpeg(), Format::Flac, hub.clone()).unwrap();
    for piece in pcm.chunks(44100 / 4 * 2).take(12) {
        encoder.write(piece).await.unwrap();
    }
    // wait until the header is there
    for _ in 0..100 {
        if !hub.header().is_empty() && hub.buffered_bytes() > 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let (listener, _) = hub.subscribe_at();
    let first = listener.try_recv().expect("the header");
    assert_eq!(&first[..4], b"fLaC");
    assert_eq!(frames::flac_header_length(&first), Ok(Some(first.len())));
    let start = listener.try_recv().expect("the latest audio");
    assert_eq!(frames::flac_start(&start), 0, "it starts on a frame");
    encoder.stop().await;
}
