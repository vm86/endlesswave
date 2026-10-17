//! The readers of encoded streams against the Python radio's (`tools/gen_golden.py`, `tests/golden/stream/`).
//! Task 3.3 of docs/rust-port.md.
//!
//! The streams (`*.bin`) are made by the radio's own ffmpeg commands and are not in git; a test without them is
//! skipped, unless `GOLDEN_REQUIRED` is set.

use radio_core::stream::frames::{self, NotThisStream, OggPage};
use serde_json::Value;
use std::{fs, path::PathBuf};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/stream")
}

fn golden() -> Value {
    serde_json::from_str(&fs::read_to_string(dir().join("frames.json")).unwrap()).unwrap()
}

fn stream(name: &str) -> Option<Vec<u8>> {
    let Ok(data) = fs::read(dir().join(format!("{name}.bin"))) else {
        assert!(std::env::var_os("GOLDEN_REQUIRED").is_none(), "{name}: run tools/gen_golden.py");
        eprintln!("{name}: no stream, skipped");
        return None;
    };
    Some(data)
}

fn u(value: &Value) -> usize {
    value.as_u64().expect("a number") as usize
}

/// Python's answer of `ogg_page_length`: None, 0 or a length.
fn page(value: &Value) -> OggPage {
    match value {
        Value::Null => OggPage::Incomplete,
        v if v.as_u64() == Some(0) => OggPage::Invalid,
        v => OggPage::Complete(u(v)),
    }
}

/// Python's answer of a header length: None, "error" or a length.
fn header(got: Result<Option<usize>, NotThisStream>) -> Value {
    match got {
        Ok(Some(n)) => n.into(),
        Ok(None) => Value::Null,
        Err(NotThisStream) => "error".into(),
    }
}

#[test]
fn mp3_frames_start_where_python_finds_them() {
    let Some(data) = stream("mp3") else { return };
    let golden = golden();
    let entry = &golden["streams"]["mp3"];
    assert_eq!(data.len(), u(&entry["length"]));
    let cuts = entry["cuts"].as_array().unwrap();
    assert!(cuts.len() > 50);
    for cut in cuts {
        let part = &data[u(&cut["begin"])..u(&cut["end"])];
        assert_eq!(frames::mp3_start(part), u(&cut["start"]), "cut {}..{}", cut["begin"], cut["end"]);
        assert_eq!(frames::mp3_frame_length(&part[..part.len().min(4)]), u(&cut["frame"]));
    }
}

#[test]
fn flac_frames_and_header_are_the_pythons() {
    let Some(data) = stream("flac") else { return };
    let golden = golden();
    let entry = &golden["streams"]["flac"];
    for cut in entry["cuts"].as_array().unwrap() {
        let part = &data[u(&cut["begin"])..u(&cut["end"])];
        assert_eq!(frames::flac_start(part), u(&cut["start"]), "cut {}..{}", cut["begin"], cut["end"]);
    }
    assert_eq!(header(frames::flac_header_length(&data)), entry["header"]);
    for h in entry["headers"].as_array().unwrap() {
        let size = u(&h["size"]);
        assert_eq!(header(frames::flac_header_length(&data[..size])), h["result"], "header of {size} bytes");
    }
}

#[test]
fn ogg_pages_and_header_are_the_pythons() {
    let Some(data) = stream("ogg") else { return };
    let golden = golden();
    let entry = &golden["streams"]["ogg"];
    for cut in entry["cuts"].as_array().unwrap() {
        let part = &data[u(&cut["begin"])..u(&cut["end"])];
        assert_eq!(frames::ogg_start(part), u(&cut["start"]), "cut {}..{}", cut["begin"], cut["end"]);
        let places = [0, 1, 5, part.len().wrapping_sub(1), part.len(), part.len() + 3];
        for (place, want) in places.iter().zip(cut["page"].as_array().unwrap()) {
            // len - 1 of an empty cut is -1 in Python, which Rust has no place for.
            if *place > part.len() + 3 {
                continue;
            }
            assert_eq!(frames::ogg_page_length(part, *place), page(want), "page at {place} of {}..{}", cut["begin"], cut["end"]);
        }
    }
    assert_eq!(header(frames::ogg_header_length(&data)), entry["header"]);
    for h in entry["headers"].as_array().unwrap() {
        let size = u(&h["size"]);
        assert_eq!(header(frames::ogg_header_length(&data[..size])), h["result"], "header of {size} bytes");
    }
}

#[test]
fn bytes_that_are_no_stream_give_what_python_gives() {
    let golden = golden();
    let randoms = golden["random"].as_array().unwrap();
    assert_eq!(randoms.len(), 40);
    for (n, case) in randoms.iter().enumerate() {
        let hex = case["hex"].as_str().unwrap();
        let data: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
        let what = format!("random {n} ({} bytes)", data.len());
        assert_eq!(frames::mp3_start(&data), u(&case["mp3_start"]), "{what}");
        assert_eq!(frames::flac_start(&data), u(&case["flac_start"]), "{what}");
        assert_eq!(frames::ogg_start(&data), u(&case["ogg_start"]), "{what}");
        assert_eq!(header(frames::flac_header_length(&data)), case["flac_header"], "{what}");
        assert_eq!(header(frames::ogg_header_length(&data)), case["ogg_header"], "{what}");
        let places = [0, 1, data.len(), data.len() + 2];
        for (place, want) in places.iter().zip(case["ogg_page"].as_array().unwrap()) {
            assert_eq!(frames::ogg_page_length(&data, *place), page(want), "{what}: page at {place}");
        }
    }
}
