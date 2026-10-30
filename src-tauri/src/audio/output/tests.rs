use super::*;
use crate::dj::Dj;
use radio_core::radio::TrackInfo;
use std::sync::mpsc::{self, TryRecvError};

fn track(id: &str) -> TrackInfo {
    TrackInfo {
        track_id: id.into(),
        title: id.into(),
        bpm: 120.0,
        duration_seconds: 20.0,
        ..TrackInfo::default()
    }
}
fn signal(start: usize, frames: usize) -> Vec<f32> {
    (start..start + frames)
        .flat_map(|n| {
            let phase = std::f64::consts::TAU * 440.0 * n as f64 / RATE as f64;
            [
                (phase.sin() * 0.2) as f32,
                ((phase + 0.8).sin() * 0.13) as f32,
            ]
        })
        .collect()
}
fn mapping(at: f64, source: f64) -> PlaybackInfo {
    PlaybackInfo {
        track: track("current"),
        next: Some(track("next")),
        mix_position: at,
        position: source,
        speed: 1.0,
        acceleration: 0.0,
        valid_until: at + 0.25,
    }
}
fn input(samples: Vec<f32>, start: f64) -> Input {
    let (tx, rx) = mpsc::channel();
    tx.send(Chunk::Pcm(samples)).unwrap();
    Input::new(
        rx,
        start,
        "listener".into(),
        Arc::new(AtomicBool::new(false)),
    )
}
fn setup() -> (Output, Dj, Arc<Shared>, Sender<Switch>) {
    let shared = Arc::new(Shared::default());
    shared.volume.store(1.0f32.to_bits(), Ordering::SeqCst);
    let dj = Dj::default();
    let (tx, rx) = mpsc::channel();
    let mut output = Output::new(shared.clone(), rx, dj.source());
    let (ack, applied) = mpsc::channel();
    tx.send(Switch::Radio {
        input: input(signal(0, RATE as usize * 20), 0.0),
        align: None,
        paused: false,
        ack,
    })
    .unwrap();
    output.fill();
    applied.recv().unwrap().unwrap();
    (output, dj, shared, tx)
}
fn render(output: &mut Output, blocks: usize) -> Vec<f32> {
    let mut pcm = Vec::new();
    for _ in 0..blocks {
        output.fill();
        pcm.extend_from_slice(&output.block);
    }
    pcm
}
fn enter(output: &mut Output, dj: &Dj, tx: &Sender<Switch>) -> f64 {
    let at = output.radio.as_ref().unwrap().position();
    let map = mapping(at, at);
    dj.prepare(
        &map,
        signal(0, RATE as usize * 20),
        Some(signal(0, RATE as usize * 20)),
        1.0,
    );
    let (ack, applied) = mpsc::channel();
    tx.send(Switch::Dj {
        mapping: map,
        playing: true,
        ack,
    })
    .unwrap();
    let samples = render(output, 4);
    applied.recv().unwrap().unwrap();
    assert!(samples.iter().all(|x| x.is_finite()));
    at
}
fn continuous(pcm: &[f32]) {
    for block in pcm.chunks(BLOCK * 2) {
        let rms =
            (block.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / block.len() as f64).sqrt();
        assert!(rms > 0.01, "the mode switch inserted silence: {rms}");
    }
    assert!(
        pcm.chunks_exact(2)
            .map(|f| f[0])
            .collect::<Vec<_>>()
            .windows(2)
            .all(|w| (w[1] - w[0]).abs() < 0.03)
    );
}

#[test]
fn entering_dj_keeps_the_waveform_position_and_preloads_the_next_deck() {
    let (mut output, dj, shared, tx) = setup();
    render(&mut output, 100);
    let at = output.radio.as_ref().unwrap().position();
    let start = output.radio.as_ref().unwrap().samples as usize / 2;
    let map = mapping(at, at);
    dj.prepare(
        &map,
        signal(0, RATE as usize * 20),
        Some(signal(0, RATE as usize * 20)),
        1.0,
    );
    let (ack, applied) = mpsc::channel();
    tx.send(Switch::Dj {
        mapping: map,
        playing: true,
        ack,
    })
    .unwrap();
    let got = render(&mut output, 12);
    applied.recv().unwrap().unwrap();
    let expected = signal(start, 12 * BLOCK);
    let error = got
        .iter()
        .zip(&expected)
        .map(|(&a, &b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(error < 0.0001, "waveform changed at the handoff: {error}");
    continuous(&got);
    assert!(shared.dj.load(Ordering::SeqCst));
    let status = serde_json::to_value(dj.status()).unwrap();
    assert_eq!(status["decks"][0]["track"]["track_id"], "current");
    assert_eq!(status["decks"][1]["track"]["track_id"], "next");
    assert_eq!(status["decks"][1]["playing"], false);
    assert_eq!(status["decks"][1]["position"], 0.0);
    assert!(status["decks"][0]["position"].as_f64().unwrap() > at);
    assert_eq!(status["crossfader"], 0.0);
}

#[test]
fn returning_to_radio_skips_old_preroll_without_interrupting_the_deck() {
    let (mut output, dj, shared, tx) = setup();
    render(&mut output, 100);
    enter(&mut output, &dj, &tx);
    render(&mut output, 100);
    let point = dj.return_point().unwrap();
    let resume = mapping(100.0, point.position);
    let mut samples = vec![0.0; RATE as usize * 2];
    samples.extend(signal(
        (point.position * RATE as f64).round() as usize,
        RATE as usize * 4,
    ));
    let (ack, applied) = mpsc::channel();
    tx.send(Switch::Radio {
        input: input(samples, 99.0),
        align: Some(resume),
        paused: false,
        ack,
    })
    .unwrap();
    let got = render(&mut output, 40);
    applied.recv().unwrap().unwrap();
    continuous(&got);
    assert!(!shared.dj.load(Ordering::SeqCst));
    assert!(!dj.is_running());
    assert!(output.radio.as_ref().unwrap().position() >= 100.0);
    assert!(shared.samples.load(Ordering::SeqCst) >= RATE as u64 * 2);
}

#[test]
fn a_slow_radio_preparation_does_not_silence_or_restart_dj() {
    let (mut output, dj, shared, tx) = setup();
    enter(&mut output, &dj, &tx);
    let point = dj.return_point().unwrap();
    let (data, chunks) = mpsc::channel();
    let new_input = Input::new(chunks, 99.0, "new".into(), Arc::new(AtomicBool::new(false)));
    let (ack, applied) = mpsc::channel();
    tx.send(Switch::Radio {
        input: new_input,
        align: Some(mapping(100.0, point.position)),
        paused: false,
        ack,
    })
    .unwrap();
    continuous(&render(&mut output, 30));
    assert!(shared.dj.load(Ordering::SeqCst));
    assert!(matches!(applied.try_recv(), Err(TryRecvError::Empty)));
    assert!(dj.return_point().unwrap().position > point.position);
    let mut samples = vec![0.0; RATE as usize * 2];
    samples.extend(signal(
        (point.position * RATE as f64).round() as usize,
        RATE as usize * 5,
    ));
    data.send(Chunk::Pcm(samples)).unwrap();
    continuous(&render(&mut output, 40));
    applied.recv().unwrap().unwrap();
    assert!(!shared.dj.load(Ordering::SeqCst));
}

#[test]
fn a_failed_handoff_leaves_the_dj_deck_playing() {
    let (mut output, dj, shared, tx) = setup();
    enter(&mut output, &dj, &tx);
    let before = dj.return_point().unwrap().position;
    let mut map = mapping(100.0, before);
    map.track.track_id = "wrong".into();
    let (ack, applied) = mpsc::channel();
    tx.send(Switch::Radio {
        input: input(vec![0.0; 1000], 99.0),
        align: Some(map),
        paused: false,
        ack,
    })
    .unwrap();
    continuous(&render(&mut output, 10));
    assert!(applied.recv().unwrap().is_err());
    assert!(shared.dj.load(Ordering::SeqCst));
    assert!(dj.return_point().unwrap().position > before);
}

#[test]
fn handback_uses_the_audible_b_deck_and_does_not_requeue_heard_a() {
    let (mut output, dj, _, tx) = setup();
    enter(&mut output, &dj, &tx);
    dj.set("b.play", 1.0).unwrap();
    dj.set("xfade", 1.0).unwrap();
    render(&mut output, 20);
    let point = dj.return_point().unwrap();
    assert_eq!(point.track.track_id, "next");
    assert!(point.playing);
    assert!(point.next.is_none(), "A has already been heard");
    assert!(point.position > 0.0);
}

#[test]
fn another_round_trip_uses_the_latest_position_instead_of_restarting_a() {
    let (mut output, dj, _, tx) = setup();
    render(&mut output, 100);
    enter(&mut output, &dj, &tx);
    render(&mut output, 100);
    let point = dj.return_point().unwrap();
    let (ack, applied) = mpsc::channel();
    tx.send(Switch::Radio {
        input: input(
            signal(
                (point.position * RATE as f64).round() as usize,
                RATE as usize * 5,
            ),
            100.0,
        ),
        align: Some(mapping(100.0, point.position)),
        paused: false,
        ack,
    })
    .unwrap();
    render(&mut output, 12);
    applied.recv().unwrap().unwrap();
    render(&mut output, 100);
    let at = output.radio.as_ref().unwrap().position();
    let source = point.position + at - 100.0;
    let map = mapping(at, source);
    dj.prepare(
        &map,
        signal(0, RATE as usize * 20),
        Some(signal(0, RATE as usize * 20)),
        1.0,
    );
    let (ack, applied) = mpsc::channel();
    tx.send(Switch::Dj {
        mapping: map,
        playing: true,
        ack,
    })
    .unwrap();
    continuous(&render(&mut output, 12));
    applied.recv().unwrap().unwrap();
    assert!(dj.return_point().unwrap().position > source);
    assert!(source > point.position + 0.5);
}

#[test]
fn pausing_during_handback_does_not_restart_playback() {
    let (mut output, dj, shared, tx) = setup();
    enter(&mut output, &dj, &tx);
    let point = dj.return_point().unwrap();
    let (ack, applied) = mpsc::channel();
    tx.send(Switch::Radio {
        input: input(
            signal(
                (point.position * RATE as f64).round() as usize,
                RATE as usize * 4,
            ),
            100.0,
        ),
        align: Some(mapping(100.0, point.position)),
        paused: false,
        ack,
    })
    .unwrap();
    render(&mut output, 2);
    dj.pause();
    shared.paused.store(true, Ordering::SeqCst);
    assert!(render(&mut output, 8).iter().all(|&x| x == 0.0));
    applied.recv().unwrap().unwrap();
    assert!(!shared.dj.load(Ordering::SeqCst));
    assert_eq!(shared.state.load(Ordering::SeqCst), STOPPED);
    let position = output.radio.as_ref().unwrap().position();
    render(&mut output, 10);
    assert_eq!(output.radio.as_ref().unwrap().position(), position);
    shared.paused.store(false, Ordering::SeqCst);
    continuous(&render(&mut output, 10));
    assert!(output.radio.as_ref().unwrap().position() > position);
}

#[test]
fn a_late_stream_connection_cannot_take_over_the_console() {
    let (mut output, dj, shared, tx) = setup();
    enter(&mut output, &dj, &tx);
    let (ack, applied) = mpsc::channel();
    tx.send(Switch::Radio {
        input: input(vec![0.0; RATE as usize * 2], 99.0),
        align: None,
        paused: false,
        ack,
    })
    .unwrap();
    continuous(&render(&mut output, 8));
    assert!(applied.recv().unwrap().is_err());
    assert!(shared.dj.load(Ordering::SeqCst));
}

#[test]
fn entering_dj_cancels_an_older_pending_stream_connection() {
    let (mut output, dj, shared, tx) = setup();
    let at = output.radio.as_ref().unwrap().position();
    let map = mapping(at, at);
    dj.prepare(
        &map,
        signal(0, RATE as usize * 20),
        Some(signal(0, RATE as usize * 20)),
        1.0,
    );
    let (old_ack, old_applied) = mpsc::channel();
    tx.send(Switch::Radio {
        input: input(vec![0.0; RATE as usize * 2], 99.0),
        align: None,
        paused: false,
        ack: old_ack,
    })
    .unwrap();
    let (ack, applied) = mpsc::channel();
    tx.send(Switch::Dj {
        mapping: map,
        playing: true,
        ack,
    })
    .unwrap();
    let (late_ack, late_applied) = mpsc::channel();
    tx.send(Switch::Radio {
        input: input(vec![0.0; RATE as usize * 2], 99.0),
        align: None,
        paused: false,
        ack: late_ack,
    })
    .unwrap();
    continuous(&render(&mut output, 8));
    applied.recv().unwrap().unwrap();
    assert!(late_applied.recv().unwrap().is_err());
    assert!(old_applied.recv().unwrap().is_err());
    assert!(shared.dj.load(Ordering::SeqCst));
}

#[test]
fn a_reconnected_stream_uses_its_new_coordinates_for_the_handoff() {
    let shared = Arc::new(Shared::default());
    shared.volume.store(1.0f32.to_bits(), Ordering::SeqCst);
    let dj = Dj::default();
    let (commands, controls) = mpsc::channel();
    let mut output = Output::new(shared.clone(), controls, dj.source());
    let (data, chunks) = mpsc::channel();
    data.send(Chunk::Start(0.0)).unwrap();
    data.send(Chunk::Pcm(signal(0, BLOCK))).unwrap();
    let (ack, applied) = mpsc::channel();
    commands
        .send(Switch::Radio {
            input: Input::new(
                chunks,
                0.0,
                "same-listener".into(),
                Arc::new(AtomicBool::new(false)),
            ),
            align: None,
            paused: false,
            ack,
        })
        .unwrap();
    output.fill();
    applied.recv().unwrap().unwrap();
    assert_eq!(shared.stream_epoch.load(Ordering::SeqCst), 0);
    data.send(Chunk::Start(100.0)).unwrap();
    data.send(Chunk::Pcm(signal(RATE as usize * 5, RATE as usize * 5)))
        .unwrap();
    output.fill();
    assert_eq!(shared.stream_epoch.load(Ordering::SeqCst), 1);
    let at = output.radio.as_ref().unwrap().position();
    assert!((at - 100.0 - BLOCK as f64 / RATE as f64).abs() < 1e-9);
    let map = mapping(at, 5.0 + BLOCK as f64 / RATE as f64);
    dj.prepare(
        &map,
        signal(0, RATE as usize * 20),
        Some(signal(0, RATE as usize * 20)),
        1.0,
    );
    let (ack, applied) = mpsc::channel();
    commands
        .send(Switch::Dj {
            mapping: map,
            playing: true,
            ack,
        })
        .unwrap();
    continuous(&render(&mut output, 8));
    applied.recv().unwrap().unwrap();
    assert!(dj.return_point().unwrap().position > 5.0);
}

#[test]
fn paused_playback_stays_paused_when_entering_dj() {
    let (mut output, dj, shared, tx) = setup();
    let at = output.radio.as_ref().unwrap().position();
    let map = mapping(at, at);
    dj.prepare(
        &map,
        signal(0, RATE as usize * 20),
        Some(signal(0, RATE as usize * 20)),
        1.0,
    );
    shared.paused.store(true, Ordering::SeqCst);
    let (ack, applied) = mpsc::channel();
    tx.send(Switch::Dj {
        mapping: map,
        playing: false,
        ack,
    })
    .unwrap();
    assert!(render(&mut output, 10).iter().all(|&x| x == 0.0));
    applied.recv().unwrap().unwrap();
    assert!(!dj.return_point().unwrap().playing);
    assert!((dj.return_point().unwrap().position - at).abs() < 0.001);
}
