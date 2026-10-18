//! `flac_roundtrip FILE OUT.flac`: decode FILE, encode it with the built-in FLAC encoder.
use radio_core::stream::{broadcaster::Broadcaster, encoder::{Encoder, Format}, native::NativeEncoder};
#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pcm = radio_core::mix::decode::decode_with_ffmpeg(std::path::Path::new("ffmpeg"), std::path::Path::new(&args[1])).unwrap();
    let hub = Broadcaster::new(Format::Flac.config());
    let listener = hub.subscribe();
    let mut encoder = NativeEncoder::start(Format::Flac, hub.clone()).unwrap();
    for piece in pcm.chunks(2 * 44100 * 2) { encoder.write(piece).await.unwrap(); }
    encoder.finish().unwrap();
    let mut data = hub.header();
    while let Some(chunk) = listener.try_recv() { data.extend_from_slice(&chunk); }
    std::fs::write(&args[2], data).unwrap();
}
