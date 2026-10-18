//! `analyze FILE...`: what the analysis says of each file (the check of stage 7 against the Python radio).
fn main() {
    for path in std::env::args().skip(1) {
        let pcm = radio_core::mix::decode::decode_file(std::path::Path::new(&path)).expect("decodes");
        let analysis = radio_core::mix::analyze(&pcm);
        println!("{path}: bpm {:.1} loudness {:.2} key {} frames {}", analysis.bpm, analysis.loudness_db, analysis.key, pcm.len() / 2);
    }
}
