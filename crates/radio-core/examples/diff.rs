use radio_core::mix;
use std::fs;
fn main() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| {
            let p = e.ok()?.path();
            (p.extension()? == "json")
                .then(|| p.file_stem().unwrap().to_string_lossy().into_owned())
        })
        .collect();
    names.sort();
    for n in names {
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(format!("{dir}/{n}.json")).unwrap()).unwrap();
        let bytes = fs::read(format!("{dir}/{n}.f32")).unwrap();
        let pcm: Vec<f32> = bytes
            .chunks_exact(4)
            .flat_map(|b| {
                let s = f32::from_le_bytes(b.try_into().unwrap());
                [s, s]
            })
            .collect();
        let got = mix::analyze(&pcm);
        let w = &json["analysis"];
        println!(
            "{n:22} bpm {:8.3}/{:8.3}  head {:9.1}/{:9.1}  tail {:9.1}/{:9.1}  range {}-{}/{}-{}  sec {} {}/{} {}  LUFS {:8.3}/{:8.3} d={:+.3}",
            got.bpm,
            w["bpm"].as_f64().unwrap(),
            got.head_beat,
            w["head_beat"].as_f64().unwrap(),
            got.tail_beat,
            w["tail_beat"].as_f64().unwrap(),
            got.start,
            got.end,
            w["start"],
            w["end"],
            got.intro_end,
            got.outro_start,
            w["intro_end"],
            w["outro_start"],
            got.loudness_db,
            w["loudness_db"].as_f64().unwrap(),
            got.loudness_db - w["loudness_db"].as_f64().unwrap()
        );
    }
}
