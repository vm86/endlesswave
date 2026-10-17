//! The radio of EndlessWave, ported from the Python `yamusic-radio` (see docs/rust-port.md).
//!
//! The crate has no Tauri in it: the app starts it, the tests drive it alone.

pub mod api;
pub mod backend;
pub mod cache;
pub mod clock;
pub mod encode;
pub mod mix;
pub mod pipeline;
pub mod radio;
pub mod spectrum;
pub mod stream;
pub mod testing;
