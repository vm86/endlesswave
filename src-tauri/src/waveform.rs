//! What the console draws under the bar of a deck: the waveform (three bands, so that the kicks, the voices and the hats
//! can be told apart by eye), the grid of the beats, and the places of the track that are good to mix at. The grid and
//! the places are found by the radio (`radio_core::mix::phrase`), which mixes by them too.

use radio_core::mix::phrase;
use serde::Serialize;

const RATE: f64 = 44_100.0;
/// The columns of the waveform.
const BINS: usize = 900;
/// The band edges: the kick and the bass below the first, the voices and the body up to the second, the hats above.
const LOW_HZ: f64 = 200.0;
const MID_HZ: f64 = 2_500.0;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Marker {
    /// Seconds from the start of the track.
    pub at: f64,
    /// "start" (the first downbeat), "intro" (the intro ends and the track fills out), "breakdown" (it thins out),
    /// "drop" (it fills out again), "outro" (it thins out for good).
    pub kind: &'static str,
    pub bar: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct Overview {
    /// Per column: the low, the mid and the high band, 0 to 1 (three numbers a column).
    pub bins: Vec<f32>,
    pub duration: f64,
    /// The BPM the grid fits best, and where its first bar begins (seconds).
    pub bpm: f64,
    pub beat_offset: f64,
    pub markers: Vec<Marker>,
}

/// One pole low-pass filter.
struct Pole {
    a: f64,
    y: f64,
}

impl Pole {
    fn new(hz: f64) -> Self {
        Self {
            a: 1.0 - (-2.0 * std::f64::consts::PI * hz / RATE).exp(),
            y: 0.0,
        }
    }
    fn run(&mut self, x: f64) -> f64 {
        self.y += self.a * (x - self.y);
        self.y
    }
}

/// Analyze a track: stereo, interleaved, 44.1 kHz; `bpm` is what the radio knows of its tempo (0 when nothing).
pub fn analyze(pcm: &[f32], bpm: f64) -> Overview {
    let frames = pcm.len() / 2;
    let per_bin = (frames / BINS).max(1);
    let mut bins = vec![0f32; BINS * 3];
    let (mut low_pole, mut mid_pole) = (Pole::new(LOW_HZ), Pole::new(MID_HZ));
    for n in 0..frames {
        let x = (pcm[n * 2] as f64 + pcm[n * 2 + 1] as f64) * 0.5;
        let l = low_pole.run(x);
        let m = mid_pole.run(x) - l;
        let h = x - l - m;
        let bin = (n / per_bin).min(BINS - 1) * 3;
        for (slot, value) in [l, m, h].into_iter().enumerate() {
            bins[bin + slot] = bins[bin + slot].max(value.abs() as f32);
        }
    }
    // The bands are shown against the same scale (the bass is the broadest, as it is in the sound), a little softened so
    // that the quiet parts are not a line. The top is the loud end, not the one peak that would flatten the rest.
    let mut sorted: Vec<f32> = bins.clone();
    sorted.sort_by(f32::total_cmp);
    let top = sorted[sorted.len() * 99 / 100].max(1e-6);
    for value in &mut bins {
        *value = (*value / top).min(1.0).powf(0.7);
    }
    let mut overview = Overview {
        bins,
        duration: frames as f64 / RATE,
        bpm,
        beat_offset: 0.0,
        markers: Vec::new(),
    };
    if let Some(found) = phrase::analyze(pcm, bpm) {
        overview.bpm = found.bpm;
        overview.beat_offset = (found.first_bar / RATE) % (found.bar / RATE);
        overview.markers = found
            .markers
            .iter()
            .map(|m| Marker {
                at: m.at as f64 / RATE,
                kind: m.kind.name(),
                bar: m.bar,
            })
            .collect();
    }
    overview
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bass tone with a hat on top, quiet in the second half.
    fn track() -> Vec<f32> {
        (0..(RATE as usize * 10))
            .flat_map(|n| {
                let t = n as f64 / RATE;
                let level = if t < 5.0 { 0.8 } else { 0.1 };
                let s = (level * ((2.0 * std::f64::consts::PI * 60.0 * t).sin() + 0.1 * (2.0 * std::f64::consts::PI * 9000.0 * t).sin())) as f32;
                [s, s]
            })
            .collect()
    }

    #[test]
    fn the_waveform_has_a_column_for_each_band() {
        let overview = analyze(&track(), 120.0);
        assert_eq!(overview.bins.len(), BINS * 3);
        // The bass is the broadest: its column reaches the top, the hats are well below it.
        let band = |slot: usize, range: std::ops::Range<usize>| range.map(|b| overview.bins[b * 3 + slot]).fold(0f32, f32::max);
        assert!(band(0, 0..400) > 0.9);
        assert!(band(2, 0..400) < band(0, 0..400));
        assert!(overview.bins.iter().all(|v| (0.0..=1.0).contains(v)));
        // The second half is quieter.
        assert!(band(0, 600..900) < band(0, 0..400));
    }

    #[test]
    fn a_track_without_a_known_tempo_has_a_waveform_and_no_markers() {
        let overview = analyze(&track(), 0.0);
        assert!(overview.markers.is_empty());
        assert_eq!(overview.bins.len(), BINS * 3);
    }
}
