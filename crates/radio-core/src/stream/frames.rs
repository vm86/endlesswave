//! Where a listener can join an encoded stream: the first whole frame of MP3 or FLAC, the first page of Ogg, and
//! how long the header of a FLAC or Ogg stream is. Task 3.3 of docs/rust-port.md, ported from `dj.py`:
//! `mp3_frame_length`, `mp3_start`, `flac_header_length`, `flac_start`, `ogg_page_length`, `ogg_header_length`,
//! `ogg_start`.

// MPEG-1 Layer III, which is what ffmpeg's libmp3lame makes of 44.1 kHz audio.
const MP3_BITRATES_KBPS: [usize; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
const MP3_SAMPLE_RATES: [usize; 3] = [44100, 48000, 32000];
const MP3_HEADER_SIZE: usize = 4;
const MP3_SYNC: u8 = 0xFF;
/// The second byte of a header without its last bit ("protection"): MPEG-1 (the two version bits), Layer III (the two
/// layer bits).
const MP3_MPEG1_LAYER3: u8 = 0xFA;
const MP3_PROTECTION_BIT: u8 = 0xFE;

/// The data is not the kind of stream that was expected (a FLAC stream that does not start with `fLaC`, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotThisStream;

/// The place of the first `needle` at or after `from`.
fn find(data: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from > data.len() {
        return None;
    }
    data[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// The length of the MP3 frame that starts like this, 0 if none. Only MPEG-1 Layer III, the kind this radio makes.
pub fn mp3_frame_length(header: &[u8]) -> usize {
    if header.len() < MP3_HEADER_SIZE || header[0] != MP3_SYNC {
        return 0;
    }
    // Frame sync, MPEG-1, Layer III (the last bit is "no CRC" or "CRC").
    if header[1] & MP3_PROTECTION_BIT != MP3_MPEG1_LAYER3 {
        return 0;
    }
    let (index, rate, padding) = (usize::from(header[2] >> 4), usize::from((header[2] >> 2) & 3), usize::from((header[2] >> 1) & 1));
    // Bitrate index 0 is "free" and 15 is invalid.
    if index == 0 || index >= MP3_BITRATES_KBPS.len() || rate >= MP3_SAMPLE_RATES.len() {
        return 0;
    }
    144 * MP3_BITRATES_KBPS[index] * 1000 / MP3_SAMPLE_RATES[rate] + padding
}

fn header_at(data: &[u8], place: usize) -> &[u8] {
    &data[place.min(data.len())..(place + MP3_HEADER_SIZE).min(data.len())]
}

/// Where the first whole MP3 frame in `data` starts. A listener can't start in the middle of a frame. A frame counts
/// when the next one follows it (the sync pattern may happen inside the audio). `data.len()` if there is none.
pub fn mp3_start(data: &[u8]) -> usize {
    let mut place = find(data, &[0xFF], 0);
    while let Some(at) = place {
        let length = mp3_frame_length(header_at(data, at));
        if length != 0 {
            let following = at + length;
            if following + MP3_HEADER_SIZE > data.len() || mp3_frame_length(header_at(data, following)) != 0 {
                return at;
            }
        }
        place = find(data, &[0xFF], at + 1);
    }
    data.len()
}

const FLAC_MARKER: &[u8] = b"fLaC";
const FLAC_BLOCK_HEADER_SIZE: usize = 4;
const FLAC_LAST_BLOCK: u8 = 0x80;
const FLAC_FRAME_SYNC: [u8; 2] = [0xFF, 0xF8]; // a fixed block size: what ffmpeg writes
const FLAC_FRAME_HEADER_MIN: usize = 6;
const FLAC_FRAME_HEADER_MAX: usize = 16;
const FLAC_UTF8_LEAD_MAX: usize = 8;
const FLAC_CRC8_POLYNOMIAL: u8 = 0x07;

/// How long the header (marker and metadata) of a FLAC stream is. `Ok(None)` if `data` doesn't hold the whole of it
/// yet; an error if it is not a FLAC stream.
pub fn flac_header_length(data: &[u8]) -> Result<Option<usize>, NotThisStream> {
    if !FLAC_MARKER.starts_with(&data[..data.len().min(FLAC_MARKER.len())]) || (data.len() > FLAC_MARKER.len() && !data.starts_with(FLAC_MARKER)) {
        return Err(NotThisStream);
    }
    let mut place = FLAC_MARKER.len();
    while place + FLAC_BLOCK_HEADER_SIZE <= data.len() {
        let last = data[place] & FLAC_LAST_BLOCK != 0;
        let size = usize::from(data[place + 1]) << 16 | usize::from(data[place + 2]) << 8 | usize::from(data[place + 3]);
        place += FLAC_BLOCK_HEADER_SIZE + size;
        if last {
            return Ok((place <= data.len()).then_some(place));
        }
    }
    Ok(None)
}

fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ FLAC_CRC8_POLYNOMIAL } else { crc << 1 };
        }
    }
    crc
}

/// The length of the frame header at `place`, 0 if it is not one.
fn flac_frame_header_length(data: &[u8], place: usize) -> usize {
    let head = &data[place..(place + FLAC_FRAME_HEADER_MAX).min(data.len())];
    if head.len() < FLAC_FRAME_HEADER_MIN || head[3] & 1 != 0 {
        // too short, or the reserved bit
        return 0;
    }
    let (block_code, rate_code) = (head[2] >> 4, head[2] & 0xF);
    // The frame number is UTF-8 coded: the leading ones give its length.
    let mut lead = 0;
    while lead < FLAC_UTF8_LEAD_MAX && head[4] & (0x80 >> lead) != 0 {
        lead += 1;
    }
    if lead == 1 || lead == FLAC_UTF8_LEAD_MAX {
        return 0;
    }
    let mut size = 4 + lead.max(1);
    size += match block_code {
        6 => 1, // the block size follows
        7 => 2,
        _ => 0,
    };
    size += match rate_code {
        12 => 1, // so does the rate
        13 | 14 => 2,
        _ => 0,
    };
    if size + 1 <= head.len() { size + 1 } else { 0 }
}

/// Where the first whole FLAC frame in `data` starts. A frame counts when the CRC-8 of its header is right (the sync
/// pattern may happen inside the audio). `data.len()` if there is none.
pub fn flac_start(data: &[u8]) -> usize {
    let mut place = find(data, &FLAC_FRAME_SYNC, 0);
    while let Some(at) = place {
        let length = flac_frame_header_length(data, at);
        if length != 0 && crc8(&data[at..at + length - 1]) == data[at + length - 1] {
            return at;
        }
        place = find(data, &FLAC_FRAME_SYNC, at + 1);
    }
    data.len()
}

const OGG_MARKER: &[u8] = b"OggS";
const OGG_PAGE_HEADER_SIZE: usize = 27;
const OGG_CONTINUED: u8 = 0x01;
const OGG_BEGINNING: u8 = 0x02;
const OGG_CRC_POLYNOMIAL: u32 = 0x04C1_1DB7;

fn ogg_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for (byte, slot) in table.iter_mut().enumerate() {
        let mut crc = (byte as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ OGG_CRC_POLYNOMIAL } else { crc << 1 };
        }
        *slot = crc;
    }
    table
}

fn ogg_crc(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(ogg_crc_table);
    data.iter().fold(0u32, |crc, &byte| (crc << 8) ^ table[((crc >> 24) as u8 ^ byte) as usize])
}

/// What there is at a place of an Ogg stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OggPage {
    /// `data` ends before the page does.
    Incomplete,
    /// There is no page: the marker, the version or the CRC is wrong (the marker may happen inside the audio).
    Invalid,
    /// A page of this length.
    Complete(usize),
}

/// The length of the Ogg page at `place`. A page counts when its CRC is right.
pub fn ogg_page_length(data: &[u8], place: usize) -> OggPage {
    // Python compares data[place:place+4] with OGG_MARKER[:len(data) - place]; the cut is from the end when the
    // place is beyond the data.
    let wanted = {
        let n = data.len() as i64 - place as i64;
        let end = if n >= 0 { (n as usize).min(4) } else { (4 + n).max(0) as usize };
        &OGG_MARKER[..end]
    };
    if data.get(place..(place + 4).min(data.len())).unwrap_or(&[]) != wanted {
        return OggPage::Invalid;
    }
    if data.len() < place + OGG_PAGE_HEADER_SIZE {
        return OggPage::Incomplete;
    }
    if data[place + 4] != 0 {
        // the version
        return OggPage::Invalid;
    }
    let segments = usize::from(data[place + 26]);
    if data.len() < place + OGG_PAGE_HEADER_SIZE + segments {
        return OggPage::Incomplete;
    }
    let table = &data[place + OGG_PAGE_HEADER_SIZE..place + 27 + segments];
    let size = OGG_PAGE_HEADER_SIZE + segments + table.iter().map(|&s| usize::from(s)).sum::<usize>();
    if data.len() < place + size {
        return OggPage::Incomplete;
    }
    let mut page = data[place..place + size].to_vec();
    let stored = u32::from_le_bytes([page[22], page[23], page[24], page[25]]);
    page[22..26].fill(0);
    if ogg_crc(&page) == stored { OggPage::Complete(size) } else { OggPage::Invalid }
}

fn ogg_granule(data: &[u8], place: usize) -> i64 {
    i64::from_le_bytes(data[place + 6..place + 14].try_into().expect("eight bytes"))
}

/// How long the header pages of an Ogg stream are: the pages before the first one of audio, which says where it is in
/// the stream (its granule position) when the header's say nothing. `Ok(None)` if `data` doesn't hold them and that
/// page yet.
pub fn ogg_header_length(data: &[u8]) -> Result<Option<usize>, NotThisStream> {
    let mut place = 0;
    loop {
        match ogg_page_length(data, place) {
            OggPage::Incomplete => return Ok(None),
            OggPage::Invalid => return Err(NotThisStream),
            OggPage::Complete(size) => {
                if place != 0 && ogg_granule(data, place) != 0 {
                    return Ok(Some(place));
                }
                place += size;
            }
        }
    }
}

/// Where the first whole page of audio in `data` starts. A listener can start on a page that doesn't go on from the
/// one before it. `data.len()` if there is none.
pub fn ogg_start(data: &[u8]) -> usize {
    let mut place = find(data, OGG_MARKER, 0);
    while let Some(at) = place {
        if let OggPage::Complete(_) = ogg_page_length(data, at) {
            if data[at + 5] & (OGG_CONTINUED | OGG_BEGINNING) == 0 && ogg_granule(data, at) > 0 {
                return at;
            }
        }
        place = find(data, OGG_MARKER, at + 1);
    }
    data.len()
}
