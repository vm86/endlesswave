//! `analyze FILE...`: what the analysis says of each file, decoded the way the radio decodes (the peak memory of stage 8
//! is measured with `/usr/bin/time -l`).
fn main() {
    for path in std::env::args().skip(1) {
        let pcm =
            radio_core::mix::decode::decode_file(std::path::Path::new(&path)).expect("decodes");
        let analysis = radio_core::mix::analyze(&pcm);
        println!(
            "{path}: bpm {:.1} loudness {:.2} key {} frames {}",
            analysis.bpm,
            analysis.loudness_db,
            analysis.key,
            pcm.len() / 2
        );
    }
}
