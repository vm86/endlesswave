//! Tool, not a test: how the grid of the phrases agrees with the analysis on the tracks the radio has downloaded.
use radio_core::mix::{self, decode, phrase};
use std::path::Path;

#[test]
#[ignore]
fn grids_of_real_tracks() {
    let dir = Path::new(&std::env::var("HOME").unwrap()).join(".cache/endlesswave-tracks");
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        let Ok(pcm) = decode::decode_file(&path) else {
            continue;
        };
        let analysis = mix::analyze(&pcm);
        let Some(p) = phrase::analyze_on(&pcm, &analysis) else {
            eprintln!(
                "{}: no phrases (bpm {}, beats {} .. {})",
                path.file_name().unwrap().to_string_lossy(),
                analysis.bpm,
                analysis.head_beat,
                analysis.tail_beat
            );
            continue;
        };
        let kinds: Vec<String> = p
            .markers
            .iter()
            .map(|m| format!("{}@{:.0}", m.kind.name(), m.at as f64 / 44100.0))
            .collect();
        eprintln!(
            "{:<30} bpm {:6.2} (grid {:6.2}) {:.0}s  {}",
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .chars()
                .take(30)
                .collect::<String>(),
            analysis.bpm,
            p.bpm,
            pcm.len() as f64 / 88200.0,
            kinds.join(" ")
        );
    }
}

/// Opt-in smoke check against a bounded sample of the local download cache.
#[test]
#[ignore]
fn automatic_phrase_plans_on_real_pairs() {
    use radio_core::mix::plan;
    let dir = Path::new(&std::env::var("HOME").unwrap()).join(".cache/endlesswave-tracks");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    paths.sort();
    let mut tracks = Vec::new();
    for path in paths.into_iter().take(6) {
        let pcm = decode::decode_file(&path).unwrap();
        let a = mix::analyze(&pcm);
        let Some(p) = phrase::analyze_on(&pcm, &a) else {
            eprintln!(
                "sample {}: {:.1} BPM, no phrase grid: fallback",
                path.file_name().unwrap().to_string_lossy(),
                a.bpm
            );
            continue;
        };
        eprintln!(
            "sample {}: {:.1} BPM, {:.0}s, {} boundaries",
            path.file_name().unwrap().to_string_lossy(),
            a.bpm,
            a.end as f64 / 44100.0,
            p.boundaries(a.start, a.end).len()
        );
        tracks.push((a, p));
    }
    let (mut matched, mut planned) = (0, 0);
    for (i, (a, out)) in tracks.iter().enumerate() {
        for (j, (b, into)) in tracks.iter().enumerate() {
            if i == j {
                continue;
            }
            let speed = plan::transition_speed(a.bpm, b);
            if speed == 0.0 {
                continue;
            }
            matched += 1;
            if let Ok(p) = plan::plan_phrase(a, out, b, into, speed, a.start) {
                assert!(p.start >= a.start && p.start + p.length <= a.end);
                assert!(p.following_start >= b.start);
                assert!(p.following_start as f64 + p.length as f64 * speed <= b.end as f64 + 1.0);
                planned += 1;
            }
        }
    }
    eprintln!("real pairs: {planned}/{matched} phrase plans; others use fallback");
    assert!(matched > 0 && planned > 0);
}
