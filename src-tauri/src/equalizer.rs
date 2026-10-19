//! The equalizer of the native player: ten bands, an octave apart, each a peaking filter (the biquad of the Audio EQ
//! Cookbook). The gains are the listener's own, set in the window; the sound of the radio for the others is not touched.

use std::f32::consts::PI;

/// The centre of each band, in Hz.
pub const BANDS: [f32; 10] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];
/// The most a band can be lifted or cut, in dB.
pub const MAX_GAIN_DB: f32 = 12.0;
/// A band spans about an octave.
const Q: f32 = 1.41;

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    /// The state of the filter (transposed direct form II).
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn peaking(rate: f32, frequency: f32, gain_db: f32) -> Option<Self> {
        // A band at or above the Nyquist frequency does nothing; nor does one with no gain.
        if gain_db == 0.0 || frequency >= rate * 0.45 {
            return None;
        }
        let a = 10f32.powf(gain_db / 40.0);
        let w = 2.0 * PI * frequency / rate;
        let alpha = w.sin() / (2.0 * Q);
        let (cos, a0) = (w.cos(), 1.0 + alpha / a);
        Some(Self {
            b0: (1.0 + alpha * a) / a0,
            b1: -2.0 * cos / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha / a) / a0,
            z1: 0.0,
            z2: 0.0,
        })
    }

    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// The equalizer of one listening: the filters of each channel, and the gains they were made for.
pub struct Equalizer {
    rate: f32,
    channels: usize,
    gains: [f32; BANDS.len()],
    /// For each channel the filters of the bands that have a gain.
    filters: Vec<Vec<Biquad>>,
    /// Against clipping: lifting a band lowers the whole.
    preamp: f32,
}

impl Equalizer {
    pub fn new(rate: u32, channels: u16) -> Self {
        let mut eq = Self {
            rate: rate as f32,
            channels: channels.max(1) as usize,
            gains: [0.0; BANDS.len()],
            filters: Vec::new(),
            preamp: 1.0,
        };
        eq.rebuild();
        eq
    }

    /// Take new gains (dB, one for each band; missing ones are 0, the rest is cut to the range).
    pub fn set_gains(&mut self, gains: &[f32]) {
        let mut next = [0.0; BANDS.len()];
        for (slot, gain) in next.iter_mut().zip(gains) {
            *slot = if gain.is_finite() {
                gain.clamp(-MAX_GAIN_DB, MAX_GAIN_DB)
            } else {
                0.0
            };
        }
        if next != self.gains {
            self.gains = next;
            self.rebuild();
        }
    }

    fn rebuild(&mut self) {
        let lift = self.gains.iter().copied().fold(0.0f32, f32::max);
        self.preamp = 10f32.powf(-lift * 0.5 / 20.0);
        let bands: Vec<Biquad> = BANDS
            .iter()
            .zip(&self.gains)
            .filter_map(|(&f, &g)| Biquad::peaking(self.rate, f, g))
            .collect();
        self.filters = vec![bands; self.channels];
    }

    /// Whether the sound is left as it is.
    pub fn is_flat(&self) -> bool {
        self.filters.first().is_none_or(Vec::is_empty)
    }

    /// Filter interleaved samples in place.
    pub fn process(&mut self, samples: &mut [f32]) {
        if self.is_flat() {
            return;
        }
        let channels = self.channels;
        for (i, sample) in samples.iter_mut().enumerate() {
            let mut x = *sample * self.preamp;
            for filter in &mut self.filters[i % channels] {
                x = filter.run(x);
            }
            *sample = x.clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(frequency: f32, rate: u32, seconds: f32) -> Vec<f32> {
        (0..(rate as f32 * seconds) as usize)
            .map(|i| 0.1 * (2.0 * PI * frequency * i as f32 / rate as f32).sin())
            .collect()
    }

    /// The level of the second half (the filter has settled), as the root mean square.
    fn level(samples: &[f32]) -> f32 {
        let half = &samples[samples.len() / 2..];
        (half.iter().map(|s| s * s).sum::<f32>() / half.len() as f32).sqrt()
    }

    #[test]
    fn flat_gains_leave_the_sound_alone() {
        let mut eq = Equalizer::new(44100, 2);
        let original = sine(1000.0, 44100, 0.5);
        let mut samples = original.clone();
        eq.process(&mut samples);
        assert_eq!(samples, original);
        assert!(eq.is_flat());
    }

    #[test]
    fn a_lifted_band_lifts_its_own_frequency_and_not_a_far_one() {
        let mut gains = [0.0; 10];
        gains[5] = 12.0; // 1 kHz
        let rate = 44100;
        let mut at_band = sine(1000.0, rate, 0.5);
        let mut far = sine(125.0, rate, 0.5);
        let (before_band, before_far) = (level(&at_band), level(&far));
        for samples in [&mut at_band, &mut far] {
            let mut eq = Equalizer::new(rate, 1);
            eq.set_gains(&gains);
            eq.process(samples);
        }
        // +12 dB is x4, less the preamp that makes room for it (6 dB, x0.5): x2 at the band.
        let lift = level(&at_band) / before_band;
        assert!((lift - 2.0).abs() < 0.15, "at the band: x{lift}");
        let far_change = level(&far) / before_far;
        assert!(
            far_change < 0.6,
            "far from the band: x{far_change} (only the preamp)"
        );
    }

    #[test]
    fn a_cut_band_cuts() {
        let mut gains = [0.0; 10];
        gains[5] = -12.0;
        let mut eq = Equalizer::new(48000, 1);
        eq.set_gains(&gains);
        let mut samples = sine(1000.0, 48000, 0.5);
        let before = level(&samples);
        eq.process(&mut samples);
        let change = level(&samples) / before;
        assert!((change - 0.25).abs() < 0.03, "x{change}");
    }

    #[test]
    fn gains_out_of_range_are_cut_and_the_sound_never_clips() {
        let mut eq = Equalizer::new(44100, 2);
        eq.set_gains(&[99.0, f32::NAN, -99.0]);
        assert_eq!(eq.gains[0], MAX_GAIN_DB);
        assert_eq!(eq.gains[1], 0.0);
        assert_eq!(eq.gains[2], -MAX_GAIN_DB);
        let mut loud: Vec<f32> = (0..2000)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        eq.process(&mut loud);
        assert!(loud.iter().all(|s| s.abs() <= 1.0));
    }

    #[test]
    fn the_channels_are_filtered_apart() {
        let mut gains = [0.0; 10];
        gains[3] = 6.0;
        let mut eq = Equalizer::new(44100, 2);
        eq.set_gains(&gains);
        // silence on the right must stay silence however loud the left is
        let mut samples: Vec<f32> = (0..4000)
            .flat_map(|i| [0.5 * ((i as f32) * 0.05).sin(), 0.0])
            .collect();
        eq.process(&mut samples);
        assert!(samples.iter().skip(1).step_by(2).all(|s| *s == 0.0));
    }
}
