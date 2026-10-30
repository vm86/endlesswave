//! Signal-level regressions: peak safety, loudness, stereo linking and continuity at the solo/blend joins.

use async_trait::async_trait;
use radio_core::{
    clock::ManualClock,
    mix::{
        self, CHANNELS, SAMPLE_RATE, TrackAnalysis,
        effects::{self, NoiseSource},
        glide, loudness, plan,
    },
    pipeline::{self, BlendPlan, Closed, Deck, Piece, PieceSink},
    testing::ScriptedRadio,
};
use std::{
    f64::consts::TAU,
    sync::{Arc, Mutex},
};

const SR: usize = SAMPLE_RATE as usize;

fn tone(hz: f64, amplitude: f64, frames: usize, antiphase: bool) -> Vec<f32> {
    (0..frames)
        .flat_map(|n| {
            let x = (amplitude * (TAU * hz * n as f64 / SR as f64 + 0.37).sin()) as f32;
            [x, if antiphase { -x } else { x * 0.7 }]
        })
        .collect()
}

fn peak(pcm: &[f32]) -> f32 {
    pcm.iter().fold(0.0f32, |top, x| top.max(x.abs()))
}

fn safe(pcm: &[f32], ceiling: f32) {
    assert!(pcm.iter().all(|x| x.is_finite()));
    assert!(peak(pcm) <= ceiling, "peak {} > {ceiling}", peak(pcm));
}

fn analysis(pcm: &[f32], bpm: f64) -> TrackAnalysis {
    TrackAnalysis {
        bpm,
        head_beat: 0.0,
        tail_beat: pcm.len() as f64 / CHANNELS as f64 - SR as f64 / 2.0,
        start: 0,
        end: (pcm.len() / CHANNELS) as i64,
        loudness_db: loudness::measure_loudness(pcm),
        intro_end: -1,
        outro_start: -1,
        key: 0,
    }
}

#[test]
fn limiter_honours_the_ceiling_at_edges_and_former_block_boundaries() {
    for ceiling in [0.5, effects::LIMIT_CEILING, 0.97] {
        for frames in [1, 2, 31, 32, 63, 64, 65, 127, 128, 129, 1025] {
            let constant: Vec<_> = (0..frames).flat_map(|_| [1.5, -0.75]).collect();
            let limited = effects::limit(&constant, ceiling);
            safe(&limited, ceiling);
            for frame in limited.chunks_exact(CHANNELS) {
                assert!((frame[0] - ceiling).abs() < 1e-6);
                assert!((frame[1] + ceiling / 2.0).abs() < 1e-6);
            }
        }
        for at in [0, 1, 31, 32, 63, 64, 95, 127, 128, 255, 256, 1023, 1024] {
            let mut pulse = vec![0.1; 1025 * CHANNELS];
            pulse[at * CHANNELS] = 1.5;
            pulse[at * CHANNELS + 1] = -3.0;
            safe(&effects::limit(&pulse, ceiling), ceiling);
        }
    }
}

#[test]
fn limiter_anticipates_peaks_and_recovers_without_a_stereo_shift() {
    let mut pcm: Vec<_> = (0..SR).flat_map(|_| [0.2, -0.1]).collect();
    let at = SR / 4;
    pcm[at * CHANNELS] = 2.0;
    let limited = effects::limit(&pcm, effects::LIMIT_CEILING);
    safe(&limited, effects::LIMIT_CEILING);
    let gain: Vec<_> = limited
        .chunks_exact(CHANNELS)
        .map(|f| -f[1] / 0.1)
        .collect();
    assert_eq!(gain[0], 1.0);
    assert!(gain[at - 100] < 1.0, "the attack starts before the peak");
    assert!((gain[at] - effects::LIMIT_CEILING / 2.0).abs() < 1e-6);
    assert!(gain[at + 100] < 0.5, "release is not instantaneous");
    assert!(gain[SR - 1] > 0.999);
    assert!(gain.windows(2).all(|w| (w[1] - w[0]).abs() < 0.005));
    for (n, frame) in limited.chunks_exact(CHANNELS).enumerate() {
        if n != at {
            assert!((frame[0] + 2.0 * frame[1]).abs() < 1e-7);
        }
    }
}

#[test]
fn limiting_a_sustained_tone_does_not_flatten_its_waveform() {
    let pcm = tone(1000.0, 1.5, SR, false);
    let limited = effects::limit(&pcm, effects::LIMIT_CEILING);
    safe(&limited, effects::LIMIT_CEILING);
    let range = SR / 4 * CHANNELS..SR * 3 / 4 * CHANNELS;
    let (input, output) = (&pcm[range.clone()], &limited[range]);
    let power = input.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>();
    let fit = input
        .iter()
        .zip(output)
        .map(|(&a, &b)| f64::from(a) * f64::from(b))
        .sum::<f64>()
        / power;
    let error = input
        .iter()
        .zip(output)
        .map(|(&a, &b)| (f64::from(b) - f64::from(a) * fit).powi(2))
        .sum::<f64>();
    let relative_error = (error / (power * fit * fit)).sqrt();
    assert!(relative_error < 0.005, "waveform error {relative_error}");
}

#[test]
fn normalization_matches_loudness_without_changing_the_stereo_image() {
    for amplitude in [0.06, 0.18, 0.8] {
        let mut pcm = tone(1000.0, amplitude, SR * 2, false);
        let original = pcm.clone();
        let before = loudness::measure_loudness(&pcm);
        let gain_db = loudness::normalize(&mut pcm, before);
        let after = loudness::measure_loudness(&pcm);
        assert!(
            (after - loudness::TARGET_LUFS).abs() < 0.02,
            "{amplitude}: {after} LUFS"
        );
        assert!((after - before - gain_db).abs() < 0.02);
        safe(&pcm, effects::LIMIT_CEILING);
        let gain = 10.0f64.powf(gain_db / 20.0);
        for (&a, &b) in original.iter().zip(&pcm) {
            assert!((f64::from(b) - f64::from(a) * gain).abs() < 1e-7);
        }
    }
}

#[test]
fn normalization_respects_peak_headroom_and_does_not_amplify_silence() {
    let mut silence = vec![0.0; SR * CHANNELS];
    assert_eq!(loudness::normalize(&mut silence, -70.0), 0.0);
    assert!(silence.iter().all(|&x| x == 0.0));
    assert_eq!(loudness::normalize(&mut [], -70.0), 0.0);
    for level in [-70.0, f64::NEG_INFINITY, f64::INFINITY, f64::NAN] {
        let mut pcm = vec![0.001; 100];
        assert_eq!(loudness::normalize(&mut pcm, level), 0.0);
        assert_eq!(pcm, vec![0.001; 100]);
    }
    let mut quiet = vec![0.0001; 100];
    assert!((loudness::normalize(&mut quiet, -50.0) - 12.0).abs() < 1e-9);
    let mut dynamic = tone(1000.0, 0.06, SR * 2, false);
    dynamic[SR] = 1.2;
    let before = loudness::measure_loudness(&dynamic);
    let gain_db = loudness::normalize(&mut dynamic, before);
    safe(&dynamic, effects::LIMIT_CEILING);
    assert!(
        gain_db < 0.0,
        "the peak takes precedence over the LUFS target"
    );
    assert!(loudness::measure_loudness(&dynamic) < loudness::TARGET_LUFS);
}

#[test]
fn tempo_glides_rejoin_the_exact_source_frame_even_with_antiphase_stereo() {
    for hz in [60.0, 440.0, 997.0] {
        for speed in [0.9, 0.96, 1.04, 1.14] {
            let length = SR * 2;
            let consumed = (length as f64 * speed).round_ties_even() as usize;
            let outgoing = tone(hz, 0.8, length + glide::GLIDE_MARGIN, true);
            let incoming = tone(hz, 0.8, consumed + glide::GLIDE_MARGIN, true);
            let (out, into, grid) = glide::tempo_glide(&outgoing, &incoming, 120.0, speed, length);
            assert_eq!(out.len(), into.len());
            assert_eq!(&out[..CHANNELS], &outgoing[..CHANNELS]);
            assert_eq!(&into[..CHANNELS], &incoming[..CHANNELS]);
            assert_eq!(
                &into[into.len() - CHANNELS..],
                &incoming[(consumed - 1) * CHANNELS..consumed * CHANNELS]
            );
            assert!((grid.last().unwrap() - into.len() as f64 / CHANNELS as f64).abs() < 1.0);
            safe(&into, 0.800_001);
            for frame in into.chunks_exact(CHANNELS) {
                assert_eq!(frame[0], -frame[1]);
            }
            let natural_step = 1.6 * (std::f64::consts::PI * hz / SR as f64).sin();
            let jump = (incoming[consumed * CHANNELS] - into[into.len() - CHANNELS]).abs();
            assert!(
                f64::from(jump) <= natural_step + 1e-6,
                "{hz} Hz, {speed}: jump {jump}"
            );
        }
    }
}

#[derive(Default)]
struct Capture(Mutex<Vec<f32>>);

#[async_trait]
impl PieceSink for Capture {
    async fn send(&self, piece: Piece) -> Result<(), Closed> {
        self.0.lock().unwrap().extend_from_slice(&piece.pcm);
        Ok(())
    }
}

struct NoNoise;
impl NoiseSource for NoNoise {
    fn normal(&mut self, count: usize) -> Vec<f64> {
        vec![0.0; count]
    }
}

async fn render(
    current: &Deck,
    following: &mut Deck,
    plan: &BlendPlan,
    effects: f64,
) -> (Vec<f32>, Arc<ScriptedRadio>) {
    let radio = ScriptedRadio::new(Vec::new(), Vec::new());
    let sink = Capture::default();
    pipeline::transition(
        &*radio,
        &ManualClock::new(0.0),
        current,
        following,
        plan,
        effects,
        Arc::new(Mutex::new(NoNoise)),
        &sink,
    )
    .await
    .unwrap();
    (sink.0.into_inner().unwrap(), radio)
}

fn levelled_deck(pcm: Vec<f32>, bpm: f64) -> Deck {
    let analysis = analysis(&pcm, bpm);
    normalized_deck(pcm, analysis)
}

fn normalized_deck(mut pcm: Vec<f32>, analysis: TrackAnalysis) -> Deck {
    let gain_db = loudness::normalize(&mut pcm, analysis.loudness_db);
    Deck {
        title: "tone".into(),
        pcm: pcm.into(),
        analysis,
        gain_db,
        beat_matched: true,
        track: None,
        mood: String::new(),
        speed: 1.0,
        generation: 0,
        source: None,
    }
}

#[tokio::test]
async fn full_transitions_keep_peak_headroom_and_join_both_solo_pieces() {
    for speed in [0.96, 1.0, 1.04] {
        for effects_level in [0.0, 1.0] {
            let current = levelled_deck(tone(440.0, 0.8, SR * 5, true), 120.0);
            let mut following = levelled_deck(tone(440.0, 0.08, SR * 5, true), 120.0 / speed);
            following.speed = speed;
            let plan = BlendPlan {
                start: SR as i64,
                following_start: (SR / 2) as i64,
                length: (SR * 4) as i64,
                speed,
            };
            let (pcm, radio) = render(&current, &mut following, &plan, effects_level).await;
            safe(&pcm, effects::LIMIT_CEILING);
            assert_eq!(
                &pcm[..CHANNELS],
                &current.pcm[plan.start as usize * CHANNELS..(plan.start as usize + 1) * CHANNELS]
            );
            let end = pipeline::followed_until(&plan, &following) as usize;
            assert_eq!(
                &pcm[pcm.len() - CHANNELS..],
                &following.pcm[(end - 1) * CHANNELS..end * CHANNELS]
            );
            for edge in [&pcm[..64], &pcm[pcm.len() - 64..]] {
                assert!(
                    edge.chunks_exact(CHANNELS)
                        .map(|f| f[0])
                        .collect::<Vec<_>>()
                        .windows(2)
                        .all(|w| (w[1] - w[0]).abs() < 0.06)
                );
            }
            assert!(
                radio
                    .log
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|v| v.to_string().contains("transition bass swap")),
                "transition selection uses the levelled loudness"
            );
        }
    }
}

#[tokio::test]
#[ignore = "uses the local download cache or RADIO_QUALITY_TRACKS"]
async fn real_tracks_are_levelled_and_their_transitions_are_peak_safe() {
    let dir = std::env::var_os("RADIO_QUALITY_TRACKS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").expect("HOME"))
                .join(".cache/endlesswave-tracks")
        });
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    paths.sort();
    let mut decks = Vec::new();
    for path in paths.into_iter().take(6) {
        let pcm = mix::decode::decode_file(&path).unwrap();
        let a = mix::analyze(&pcm);
        let mut deck = normalized_deck(pcm, a);
        deck.title = path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
            .into();
        let measured = loudness::measure_loudness(&deck.pcm);
        eprintln!(
            "{}: {:.2} -> {measured:.2} LUFS, gain {:+.2} dB, peak {:.6}",
            deck.title,
            deck.analysis.loudness_db,
            deck.gain_db,
            peak(&deck.pcm)
        );
        assert!((measured - deck.analysis.loudness_db - deck.gain_db).abs() < 0.1);
        safe(&deck.pcm, effects::LIMIT_CEILING);
        decks.push(deck);
    }
    assert!(decks.len() >= 2);
    let mut rendered = 0;
    for pair in decks.windows(2) {
        let current = &pair[0];
        let mut following = pair[1].clone();
        let speed = plan::transition_speed(current.analysis.bpm, &following.analysis);
        following.beat_matched = speed != 0.0;
        following.speed = if speed == 0.0 { 1.0 } else { speed };
        let start = (current.analysis.end - (4 * SR) as i64).max(current.analysis.start);
        let plan = pipeline::plan_early(current, &following, start - (2 * SR) as i64)
            .unwrap_or_else(|| pipeline::plan_blend(current, &mut following, start));
        let (pcm, _) = render(current, &mut following, &plan, 1.0).await;
        assert!(!pcm.is_empty());
        safe(&pcm, effects::LIMIT_CEILING);
        eprintln!(
            "blend {} -> {}: {:.2}s, peak {:.6}",
            current.title,
            following.title,
            pcm.len() as f64 / (SR * CHANNELS) as f64,
            peak(&pcm)
        );
        rendered += 1;
    }
    assert!(rendered > 0);
}
