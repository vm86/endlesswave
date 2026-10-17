//! The radio of EndlessWave, ported from the Python `yamusic-radio` (see docs/rust-port.md).
//!
//! The crate has no Tauri in it: the app starts it, the tests drive it alone.

pub mod cache;
pub mod mix;
pub mod spectrum;
pub mod stream;
