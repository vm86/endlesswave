//! `analyze_mem FILE`: the resident memory after each step of the analysis (stage 8).
use radio_core::mix::{analysis, decode, estimate_key, loudness};
use std::process::Command;

fn rss(label: &str) {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    println!(
        "{label:28} {:>6} MB",
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u64>()
            .unwrap()
            / 1024
    );
}

fn main() {
    let path = std::env::args().nth(1).unwrap();
    rss("start");
    let pcm = decode::decode_file(std::path::Path::new(&path)).unwrap();
    rss("decoded");
    let (start, end) = analysis::audible_range(&pcm);
    let loud = loudness::measure_loudness(&pcm[start * 2..end * 2]);
    rss("loudness first");
    let (_rhythm, mono) = analysis::rhythm_and_mono(&pcm);
    rss("rhythm (mono kept)");
    let key = estimate_key(&mono);
    rss("key");
    println!(
        "key {key} loudness {loud:.2} pcm {} MB",
        pcm.len() * 4 / (1 << 20)
    );
}
