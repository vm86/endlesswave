//! The readers of encoded streams, with the answers of the original Python radio kept in `tests/golden/stream/`.
//! Task 3.3 of docs/rust-port.md.

use radio_core::stream::frames::{self, NotThisStream, OggPage};
use serde_json::Value;
use std::{fs, path::PathBuf};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/stream")
}

fn golden() -> Value {
    serde_json::from_str(&fs::read_to_string(dir().join("frames.json")).unwrap()).unwrap()
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
fn bytes_that_are_no_stream_give_what_python_gives() {
    let golden = golden();
    let randoms = golden["random"].as_array().unwrap();
    assert_eq!(randoms.len(), 40);
    for (n, case) in randoms.iter().enumerate() {
        let hex = case["hex"].as_str().unwrap();
        let data: Vec<u8> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
            .collect();
        let what = format!("random {n} ({} bytes)", data.len());
        assert_eq!(frames::mp3_start(&data), u(&case["mp3_start"]), "{what}");
        assert_eq!(frames::flac_start(&data), u(&case["flac_start"]), "{what}");
        assert_eq!(frames::ogg_start(&data), u(&case["ogg_start"]), "{what}");
        assert_eq!(
            header(frames::flac_header_length(&data)),
            case["flac_header"],
            "{what}"
        );
        assert_eq!(
            header(frames::ogg_header_length(&data)),
            case["ogg_header"],
            "{what}"
        );
        let places = [0, 1, data.len(), data.len() + 2];
        for (place, want) in places.iter().zip(case["ogg_page"].as_array().unwrap()) {
            assert_eq!(
                frames::ogg_page_length(&data, *place),
                page(want),
                "{what}: page at {place}"
            );
        }
    }
}
