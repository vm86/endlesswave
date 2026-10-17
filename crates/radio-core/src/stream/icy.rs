//! The titles in the MP3 stream (ICY metadata), task 3.3 of docs/rust-port.md. Ported from `icy_block` and the
//! loop of `_stream_handler` in `dj.py`.

/// The audio between two metadata blocks (`icy-metaint`).
pub const ICY_METAINT: usize = 16000;
/// `icy-name`.
pub const ICY_NAME: &str = "Yandex Music DJ";

/// An ICY metadata block: the length in 16-byte units, then the text.
pub fn icy_block(title: &str) -> Vec<u8> {
    let text = format!("StreamTitle='{}';", title.replace('\'', "\u{2019}")).into_bytes();
    let text = &text[..text.len().min(255 * 16)];
    let padding = (16 - text.len() % 16) % 16;
    let mut block = vec![((text.len() + padding) / 16) as u8];
    block.extend_from_slice(text);
    block.resize(block.len() + padding, 0);
    block
}

/// Puts the metadata into the stream of one listener who asked for it: after every `ICY_METAINT` bytes of audio there
/// is a block, empty ("\0") unless the title has changed since the last one.
#[derive(Default)]
pub struct IcyMux {
    pending: Vec<u8>,
    sent_title: Option<String>,
}

impl IcyMux {
    /// Take a chunk of audio; give what is to be written now. `title` is the one that plays at this moment.
    pub fn push(&mut self, chunk: &[u8], title: &str) -> Vec<u8> {
        self.pending.extend_from_slice(chunk);
        let mut out = Vec::new();
        while self.pending.len() >= ICY_METAINT {
            let block = if self.sent_title.as_deref() == Some(title) {
                vec![0]
            } else {
                self.sent_title = Some(title.to_owned());
                icy_block(title)
            };
            out.extend(self.pending.drain(..ICY_METAINT));
            out.extend(block);
        }
        out
    }
}
