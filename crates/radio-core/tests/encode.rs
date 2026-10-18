//! The loop that mixes, encodes and publishes, on a fake encoder and the paused clock of tokio: waiting for listeners
//! and for the end of a pause, keeping a little ahead of the listeners, telling the streams where the mix is, and
//! starting again after a failure. Tasks 3.4.1 to 3.4.3 of docs/rust-port-radio.md. (What the Python radio does here
//! depends on real time, so these tests are of the rules the Python code states, not of recorded answers.)
//! The last one runs real ffmpeg on tracks made by the test, and is skipped without ffmpeg.

use bytes::Bytes;
use radio_core::{
    clock::{Clock, SystemClock},
    encode::{
        EncodeError, Options, PACING_LEAD_SECONDS, Streams, encode_forever, encode_loop, hold,
    },
    mix::{TrackAnalysis, decode, effects::NoiseSource},
    pipeline::{MixError, SharedNoise},
    spectrum::SpectrumFeed,
    stream::{
        broadcaster::{Broadcaster, Config},
        encoder::{Encoder, FfmpegEncoder, Format},
    },
    testing::{ScriptedRadio, tone_deck},
};
use std::{
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

struct NoNoise;

impl NoiseSource for NoNoise {
    fn normal(&mut self, _count: usize) -> Vec<f64> {
        panic!("no effects, no noise");
    }
}

fn noise() -> SharedNoise {
    Arc::new(Mutex::new(NoNoise))
}

/// What the fake encoders were asked, shared among them.
#[derive(Default)]
struct Log {
    /// (format, when) of every encoder that was started.
    started: Mutex<Vec<(Format, f64)>>,
    writes: AtomicUsize,
    stopped: AtomicUsize,
}

struct FakeEncoder {
    hub: Arc<Broadcaster>,
    log: Arc<Log>,
    /// Fails when this many pieces have been written to it.
    fail_at: Option<usize>,
    written: usize,
}

impl Encoder for FakeEncoder {
    async fn write(&mut self, pcm: &[f32]) -> io::Result<()> {
        self.written += 1;
        if self.fail_at == Some(self.written) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the encoder is gone",
            ));
        }
        self.log.writes.fetch_add(1, Ordering::SeqCst);
        // The stream gets a byte for every frame, as a stand-in for what an encoder writes.
        self.hub.publish(Bytes::from(vec![0u8; pcm.len() / 2]));
        Ok(())
    }

    async fn stop(self) {
        self.log.stopped.fetch_add(1, Ordering::SeqCst);
    }
}

fn mp3_only() -> Streams {
    Streams {
        mp3: Broadcaster::new(Config::mp3()),
        flac: None,
        ogg: None,
    }
}

fn radio(seconds: f64) -> Arc<ScriptedRadio> {
    ScriptedRadio::new(
        vec![
            tone_deck("Tone A", seconds, 120.0),
            tone_deck("Tone B", seconds, 124.0),
        ],
        vec![0, 1],
    )
}

fn fake_factory(
    log: Arc<Log>,
    clock: Arc<SystemClock>,
    fail_first_at: Option<usize>,
) -> impl Fn(Format, Arc<Broadcaster>) -> io::Result<FakeEncoder> + Send + Sync {
    move |format, hub| {
        let mut started = log.started.lock().unwrap();
        // Only the first encoder to be started can be made to fail.
        let fail_at = if started.is_empty() {
            fail_first_at
        } else {
            None
        };
        started.push((format, clock.monotonic()));
        Ok(FakeEncoder {
            hub,
            log: log.clone(),
            fail_at,
            written: 0,
        })
    }
}

#[tokio::test(start_paused = true)]
async fn the_mix_waits_for_a_listener_and_goes_on_when_there_is_one() {
    let radio = radio(10.0);
    let hubs = vec![
        Broadcaster::new(Config::mp3()),
        Broadcaster::new(Config::flac()),
    ];

    let waiting = {
        let (radio, hubs) = (radio.clone(), hubs.clone());
        tokio::spawn(async move { hold(&*radio, &hubs).await })
    };
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert!(!waiting.is_finished(), "nobody listens");
    assert_eq!(
        *radio.holds.lock().unwrap(),
        [true],
        "the radio is told that the mix waits"
    );

    // a listener of the second stream is enough
    let _listener = hubs[1].subscribe();
    assert!(waiting.await.unwrap(), "it had to wait");
    assert_eq!(*radio.holds.lock().unwrap(), [true, false]);

    // with a listener there is no waiting, and the radio is not told
    radio.holds.lock().unwrap().clear();
    assert!(!hold(&*radio, &hubs).await);
    assert!(radio.holds.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_mix_waits_while_it_is_paused() {
    let radio = radio(10.0);
    let hubs = vec![Broadcaster::new(Config::mp3())];
    let _listener = hubs[0].subscribe();
    radio.set_paused(true);
    let waiting = {
        let (radio, hubs) = (radio.clone(), hubs.clone());
        tokio::spawn(async move { hold(&*radio, &hubs).await })
    };
    tokio::time::sleep(Duration::from_secs(30)).await;
    assert!(!waiting.is_finished(), "paused");
    radio.set_paused(false);
    assert!(waiting.await.unwrap());
    assert_eq!(*radio.holds.lock().unwrap(), [true, false]);
}

/// The pieces written by the loop (a quarter of a second each) when `seconds` of the clock have passed.
async fn pieces_after(seconds: u64, listener_after: u64) -> (usize, f64) {
    let log = Arc::new(Log::default());
    let clock = Arc::new(SystemClock::new());
    let streams = mp3_only();
    let hub = streams.mp3.clone();
    let loop_task = {
        let (log, clock, streams) = (log.clone(), clock.clone(), streams.clone());
        tokio::spawn(async move {
            let factory = fake_factory(log, clock.clone(), None);
            encode_loop(
                radio(120.0),
                clock,
                0.0,
                noise(),
                &streams,
                None,
                &factory,
                Options::default(),
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_secs(listener_after)).await;
    let before = log.writes.load(Ordering::SeqCst);
    assert_eq!(before, 0, "nothing is mixed for nobody");
    let _listener = hub.subscribe();
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    let written = log.writes.load(Ordering::SeqCst);
    loop_task.abort();
    (written, hub.fed())
}

#[tokio::test(start_paused = true)]
async fn the_mix_keeps_two_seconds_ahead_of_the_listeners() {
    let (written, fed) = pieces_after(10, 0).await;
    // 10 s of the clock and the lead of 2 s: 12 s of the mix, in pieces of a quarter of a second
    let expected = (10.0 + PACING_LEAD_SECONDS) / 0.25;
    assert!(
        (written as f64 - expected).abs() <= 2.0,
        "{written} pieces, {expected} expected"
    );
    assert!(
        (fed - written as f64 * 0.25).abs() < 1e-6,
        "the streams are told how far the mix is fed: {fed}"
    );
}

#[tokio::test(start_paused = true)]
async fn after_waiting_for_a_listener_the_mix_does_not_run_to_catch_up() {
    // 20 s go by with nobody listening; then, 10 s of listening give the same as at once
    let (written, _) = pieces_after(10, 20).await;
    let expected = (10.0 + PACING_LEAD_SECONDS) / 0.25;
    assert!(
        (written as f64 - expected).abs() <= 2.0,
        "{written} pieces, {expected} expected"
    );
}

#[tokio::test(start_paused = true)]
async fn the_streams_the_spectrum_and_the_title_are_told_about_each_piece() {
    let log = Arc::new(Log::default());
    let clock = Arc::new(SystemClock::new());
    let streams = Streams {
        mp3: Broadcaster::new(Config::mp3()),
        flac: Some(Broadcaster::new(Config::flac())),
        ogg: Some(Broadcaster::new(Config::ogg())),
    };
    let spectrum = SpectrumFeed::new();
    let (watcher, _) = spectrum.subscribe(None);
    let _listener = streams.ogg.as_ref().unwrap().subscribe(); // any stream's listener lets the mix go
    let task = {
        let (streams, spectrum, log, clock) = (
            streams.clone(),
            spectrum.clone(),
            log.clone(),
            clock.clone(),
        );
        tokio::spawn(async move {
            let factory = fake_factory(log, clock.clone(), None);
            encode_loop(
                radio(120.0),
                clock,
                0.0,
                noise(),
                &streams,
                Some(spectrum),
                &factory,
                Options::default(),
            )
            .await
        })
    };
    tokio::time::sleep(Duration::from_secs(3)).await;
    task.abort();

    let written = log.writes.load(Ordering::SeqCst);
    assert_eq!(
        log.started
            .lock()
            .unwrap()
            .iter()
            .map(|(f, _)| *f)
            .collect::<Vec<_>>(),
        [Format::Mp3, Format::Flac, Format::Ogg]
    );
    // every encoder gets every piece
    assert_eq!(written % 3, 0);
    let pieces = written / 3;
    assert!(pieces > 10);
    assert_eq!(
        streams.mp3.title(),
        "Tone A",
        "the title is of the first stream"
    );
    for (hub, name) in [
        (&streams.mp3, "mp3"),
        (streams.flac.as_ref().unwrap(), "flac"),
        (streams.ogg.as_ref().unwrap(), "ogg"),
    ] {
        assert!(
            (hub.fed() - pieces as f64 * 0.25).abs() < 1e-6,
            "{name}: fed {}",
            hub.fed()
        );
    }
    // the spectrum got the bands of what was fed: about 30 frames a second, the first window needing 2048 samples
    let mut frames = 0;
    while let Some(batch) = watcher.try_recv() {
        frames += batch.len();
    }
    let expected = ((pieces as f64 * 11025.0 - 2048.0) / 1470.0).floor() + 1.0;
    assert!(
        (frames as f64 - expected).abs() <= 1.0,
        "{frames} frames of the spectrum, {expected} expected"
    );
}

#[tokio::test(start_paused = true)]
async fn a_failure_ends_only_the_mix_it_happened_in() {
    let log = Arc::new(Log::default());
    let clock = Arc::new(SystemClock::new());
    let streams = mp3_only();
    let radio = radio(120.0);
    let _listener = streams.mp3.subscribe();
    // The first encoder breaks on its third piece.
    let factory = fake_factory(log.clone(), clock.clone(), Some(3));
    let task = tokio::spawn(encode_forever(
        radio.clone(),
        clock.clone(),
        0.0,
        noise(),
        streams.clone(),
        None,
        factory,
        Options::default(),
    ));
    tokio::time::sleep(Duration::from_secs(20)).await;
    task.abort();

    let started = log.started.lock().unwrap().clone();
    assert!(started.len() >= 2, "the mix started again: {started:?}");
    let gap = started[1].1 - started[0].1;
    assert!(
        (gap - 5.0).abs() < 0.5,
        "it waited about five seconds: {gap}"
    );
    assert!(
        log.stopped.load(Ordering::SeqCst) >= 1,
        "the broken encoder was stopped"
    );
    // The second mix began anew: the first deck was asked for again, with nothing before it.
    let asked = radio
        .log
        .lock()
        .unwrap()
        .iter()
        .filter(|e| {
            e[1].as_str()
                .is_some_and(|t| t.starts_with("next_deck after none"))
        })
        .count();
    assert!(asked >= 2, "{asked} mixes began");
    // and the positions began again with it
    assert!(streams.mp3.fed() < 20.0);
}

#[tokio::test(start_paused = true)]
async fn an_encoder_that_cannot_start_stops_the_ones_started() {
    let log = Arc::new(Log::default());
    let clock = Arc::new(SystemClock::new());
    let streams = Streams {
        mp3: Broadcaster::new(Config::mp3()),
        flac: Some(Broadcaster::new(Config::flac())),
        ogg: None,
    };
    let started = log.clone();
    let factory = move |format: Format, hub: Arc<Broadcaster>| -> io::Result<FakeEncoder> {
        if format == Format::Flac {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no ffmpeg"));
        }
        Ok(FakeEncoder {
            hub,
            log: started.clone(),
            fail_at: None,
            written: 0,
        })
    };
    let ended = encode_loop(
        radio(10.0),
        clock,
        0.0,
        noise(),
        &streams,
        None,
        &factory,
        Options::default(),
    )
    .await;
    assert!(
        matches!(ended, Err(EncodeError::Encoder(ref e)) if e.kind() == io::ErrorKind::NotFound),
        "{ended:?}"
    );
    assert_eq!(
        log.stopped.load(Ordering::SeqCst),
        1,
        "the MP3 encoder that was started is stopped"
    );
}

#[tokio::test(start_paused = true)]
async fn a_deck_that_cannot_be_made_ends_the_mix_and_stops_the_encoders() {
    let log = Arc::new(Log::default());
    let clock = Arc::new(SystemClock::new());
    let streams = mp3_only();
    let _listener = streams.mp3.subscribe();
    let radio = radio(10.0);
    *radio.fail_after.lock().unwrap() = Some(0);
    let factory = fake_factory(log.clone(), clock.clone(), None);
    let ended = encode_loop(
        radio,
        clock,
        0.0,
        noise(),
        &streams,
        None,
        &factory,
        Options::default(),
    )
    .await;
    assert!(
        matches!(ended, Err(EncodeError::Mix(MixError::Deck(_)))),
        "{ended:?}"
    );
    assert_eq!(log.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(log.writes.load(Ordering::SeqCst), 0);
}

/// With real ffmpeg and the library's tracks: the listener of each stream gets a stream that its own reader recognizes.
#[tokio::test]
async fn real_encoders_give_streams_a_listener_can_join() {
    if !std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        eprintln!("no ffmpeg: skipped");
        return;
    }
    let dir = std::env::temp_dir().join(format!("radio-core-encode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut decks = Vec::new();
    for i in 0..2u32 {
        let file = dir.join(format!("t{i}.wav"));
        radio_core::testing::write_music_wav(&file, 118.0 + f64::from(i) * 6.0, 50.0, i).unwrap();
        let pcm = decode::decode_with_ffmpeg(std::path::Path::new("ffmpeg"), &file).unwrap();
        let analysis: TrackAnalysis = radio_core::mix::analyze(&pcm);
        decks.push((format!("Track {i}"), Arc::<[f32]>::from(pcm), analysis));
    }
    let radio = ScriptedRadio::new(decks, vec![0, 1]);
    let streams = Streams {
        mp3: Broadcaster::new(Config::mp3()),
        flac: Some(Broadcaster::new(Config::flac())),
        ogg: Some(Broadcaster::new(Config::ogg())),
    };
    let (mp3, flac, ogg) = (
        streams.mp3.subscribe(),
        streams.flac.as_ref().unwrap().subscribe(),
        streams.ogg.as_ref().unwrap().subscribe(),
    );
    let options = Options {
        pacing_lead: 1e9,
        ..Options::default()
    };
    let task = {
        let streams = streams.clone();
        tokio::spawn(async move {
            let factory =
                |format, hub| FfmpegEncoder::start(std::path::Path::new("ffmpeg"), format, hub);
            encode_loop(
                radio,
                Arc::new(SystemClock::new()),
                0.0,
                noise(),
                &streams,
                None,
                &factory,
                options,
            )
            .await
        })
    };
    let mut received = [Vec::new(), Vec::new(), Vec::new()];
    let wanted = [150_000usize, 150_000, 150_000];
    let collect = async {
        loop {
            for (i, listener) in [&mp3, &flac, &ogg].into_iter().enumerate() {
                if received[i].len() < wanted[i] {
                    received[i].extend_from_slice(&listener.recv().await);
                }
            }
            if received.iter().zip(wanted).all(|(r, w)| r.len() >= w) {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(60), collect)
        .await
        .expect("the streams came");
    task.abort();

    use radio_core::stream::frames;
    // Who was there from the start gets the stream from its first byte: ffmpeg begins an MP3 with a tag (its size is in
    // the tag, and depends on the version of ffmpeg: it names the library), and the first frame follows it.
    assert_eq!(&received[0][..3], b"ID3");
    let tag = 10
        + received[0][6..10]
            .iter()
            .fold(0usize, |size, &b| size << 7 | usize::from(b & 0x7F));
    assert_eq!(frames::mp3_start(&received[0]), tag);
    // Who joins later gets the latest seconds from a frame.
    let (late, position) = streams.mp3.subscribe_at();
    let first = late.try_recv().expect("the latest audio at once");
    assert_eq!(frames::mp3_start(&first), 0, "it starts on a frame");
    assert!(position.is_some_and(|p| p > 0.0), "{position:?}");
    // The header of FLAC and of Ogg is kept apart, and sent first to who joins after it is known. Who was there before
    // gets it as soon as it is known (the Python radio sends such a listener the audio only).
    for (hub, marker) in [
        (streams.flac.as_ref().unwrap(), &b"fLaC"[..]),
        (streams.ogg.as_ref().unwrap(), &b"OggS"[..]),
    ] {
        let header = hub.header();
        assert_eq!(&header[..4], marker);
        let (late, _) = hub.subscribe_at();
        assert_eq!(late.try_recv().expect("the header first"), header);
        assert!(late.try_recv().is_some(), "and then the latest audio");
    }
    assert!(
        frames::flac_header_length(&streams.flac.as_ref().unwrap().header())
            .unwrap()
            .is_some()
    );
    // The Ogg header is whole pages, to the last byte.
    let ogg_header = streams.ogg.as_ref().unwrap().header();
    let mut place = 0;
    while let frames::OggPage::Complete(size) = frames::ogg_page_length(&ogg_header, place) {
        place += size;
    }
    assert_eq!(place, ogg_header.len());
    let flac_header = streams.flac.as_ref().unwrap().header();
    assert!(
        received[1].starts_with(&flac_header),
        "the early FLAC listener has the header first"
    );
    assert_eq!(
        frames::flac_start(&received[1][flac_header.len()..]),
        0,
        "the FLAC audio starts on a frame"
    );
    assert!(
        received[2].starts_with(&ogg_header),
        "the early Ogg listener has the header first"
    );
    assert_eq!(
        &received[2][ogg_header.len()..][..4],
        b"OggS",
        "the Ogg audio starts on a page"
    );
    assert_eq!(streams.mp3.title(), "Track 0");
}
