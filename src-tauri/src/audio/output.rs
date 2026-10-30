use super::{Chunk, NowPlaying, PLAYING, STOPPED, Shared};
use crate::{dj::Console, equalizer::Equalizer};
use radio_core::radio::PlaybackInfo;
use rodio::{ChannelCount, SampleRate, Source};
use std::{
    num::NonZero,
    sync::atomic::{AtomicBool, Ordering},
    sync::{
        Arc,
        mpsc::{Receiver, Sender, TryRecvError},
    },
    time::{Duration, Instant},
};

const RATE: u32 = 44_100;
const BLOCK: usize = 256;
const FADE: usize = 882;
const MAX_CATCHUP_FRAMES: usize = BLOCK * 8;

pub(super) struct Input {
    chunks: Receiver<Chunk>,
    current: Vec<f32>,
    at: usize,
    pub start: f64,
    pub samples: u64,
    pub connection: String,
    epoch: u64,
    received_start: bool,
    ended: bool,
    gap: bool,
    stop: Arc<AtomicBool>,
}

impl Input {
    pub fn new(
        chunks: Receiver<Chunk>,
        start: f64,
        connection: String,
        stop: Arc<AtomicBool>,
    ) -> Self {
        Self {
            chunks,
            current: Vec::new(),
            at: 0,
            start,
            samples: 0,
            connection,
            epoch: 0,
            received_start: false,
            ended: false,
            gap: false,
            stop,
        }
    }
    pub fn position(&self) -> f64 {
        self.start + self.samples as f64 / (RATE as f64 * 2.0)
    }
    fn sample(&mut self) -> Option<f32> {
        while self.at >= self.current.len() {
            match self.chunks.try_recv() {
                Ok(Chunk::Start(start)) => {
                    if self.received_start || self.samples > 0 {
                        self.epoch += 1;
                    }
                    self.received_start = true;
                    self.start = start;
                    self.samples = 0;
                    continue;
                }
                Ok(Chunk::Pcm(chunk)) => {
                    self.current = chunk;
                    self.at = 0;
                    self.gap = false;
                }
                Err(TryRecvError::Empty) => return None,
                Err(TryRecvError::Disconnected) => {
                    self.ended = true;
                    return None;
                }
            }
        }
        let sample = *self.current.get(self.at)?;
        self.at += 1;
        self.samples += 1;
        Some(sample)
    }
    fn heard(&mut self, shared: &Shared) -> f32 {
        match self.sample() {
            Some(sample) => sample,
            None => {
                if !self.gap {
                    shared.underruns.fetch_add(1, Ordering::Relaxed);
                    self.gap = true;
                }
                0.0
            }
        }
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

type Ack = Sender<Result<(), String>>;

pub(super) enum Switch {
    Dj {
        mapping: PlaybackInfo,
        playing: bool,
        ack: Ack,
    },
    Radio {
        input: Input,
        align: Option<PlaybackInfo>,
        paused: bool,
        ack: Ack,
    },
}

struct Pending {
    input: Input,
    align: Option<PlaybackInfo>,
    paused: bool,
    ack: Ack,
    since: Instant,
}

struct Fade {
    to_dj: bool,
    at: usize,
    ack: Ack,
}

pub(super) struct Output {
    shared: Arc<Shared>,
    commands: Receiver<Switch>,
    radio: Option<Input>,
    console: Console,
    dj: bool,
    pending: Option<Pending>,
    fade: Option<Fade>,
    block: Vec<f32>,
    at: usize,
    eq: Equalizer,
    eq_seen: u32,
}

impl Output {
    pub fn new(shared: Arc<Shared>, commands: Receiver<Switch>, console: Console) -> Self {
        Self {
            shared,
            commands,
            radio: None,
            console,
            dj: false,
            pending: None,
            fade: None,
            block: Vec::new(),
            at: 0,
            eq: Equalizer::new(RATE, 2),
            eq_seen: u32::MAX,
        }
    }

    fn controls(&mut self) {
        if self.fade.is_some() {
            return;
        }
        while let Ok(command) = self.commands.try_recv() {
            match command {
                Switch::Dj {
                    mapping,
                    playing,
                    ack,
                } => {
                    if self.dj {
                        let _ = ack.send(Ok(()));
                        continue;
                    }
                    let at = self.radio.as_ref().map(Input::position);
                    if at.is_some_and(|at| {
                        at > mapping.valid_until + 0.001 || at < mapping.mix_position - 0.25
                    }) {
                        let _ = ack.send(Err(
                            "the playback position changed; retry the handoff".into()
                        ));
                        continue;
                    }
                    if let Some(pending) = self.pending.take() {
                        let _ = pending.ack.send(Err("stopped".into()));
                    }
                    let position = at.map_or(mapping.position, |at| mapping.source_at(at));
                    let tempo = at.map_or(mapping.speed, |at| {
                        mapping.speed * (mapping.acceleration * (at - mapping.mix_position)).exp()
                    });
                    self.console.enter(position.max(0.0), tempo, playing);
                    if playing && self.radio.is_some() {
                        self.fade = Some(Fade {
                            to_dj: true,
                            at: 0,
                            ack,
                        });
                    } else {
                        self.finish(true, ack);
                    }
                    break;
                }
                Switch::Radio {
                    input,
                    align,
                    paused,
                    ack,
                } => {
                    if self.dj && align.is_none() {
                        let _ = ack.send(Err("stopped".into()));
                        continue;
                    }
                    if let Some(old) = self.pending.take() {
                        let _ = old.ack.send(Err("playback was replaced".into()));
                    }
                    self.pending = Some(Pending {
                        input,
                        align,
                        paused,
                        ack,
                        since: Instant::now(),
                    });
                }
            }
        }
    }

    fn prepare_radio(&mut self) {
        if self.fade.is_some() {
            return;
        }
        let Some(mut pending) = self.pending.take() else {
            return;
        };
        if self.dj
            && let Some(mapping) = &pending.align
        {
            let Some(point) = self.console.return_point() else {
                let _ = pending
                    .ack
                    .send(Err("there is no DJ track to continue".into()));
                return;
            };
            if point.track.track_id != mapping.track.track_id && point.playing {
                let _ = pending
                    .ack
                    .send(Err("the DJ track changed during the handoff".into()));
                return;
            }
            pending.paused = !point.playing;
            let target = if point.track.track_id == mapping.track.track_id {
                mapping.mix_position + (point.position - mapping.position).max(0.0) / mapping.speed
            } else {
                mapping.mix_position
            };
            let mut skipped = 0;
            while pending.input.position() + 1.0 / (RATE as f64) < target
                && skipped < MAX_CATCHUP_FRAMES
            {
                if pending.input.sample().is_none() {
                    break;
                }
                if pending.input.sample().is_none() {
                    break;
                }
                skipped += 1;
            }
            if pending.input.position() + 1.0 / (RATE as f64) < target {
                if pending.input.ended || pending.since.elapsed() > Duration::from_secs(15) {
                    let _ = pending
                        .ack
                        .send(Err("the radio could not catch up to the DJ deck".into()));
                } else {
                    self.pending = Some(pending);
                }
                return;
            }
        }
        self.shared.paused.store(pending.paused, Ordering::SeqCst);
        *self.shared.connection.lock().unwrap() = pending.input.connection.clone();
        self.radio = Some(pending.input);
        if self.dj && !pending.paused {
            self.fade = Some(Fade {
                to_dj: false,
                at: 0,
                ack: pending.ack,
            });
        } else {
            self.finish(false, pending.ack);
        }
    }

    fn finish(&mut self, dj: bool, ack: Ack) {
        self.dj = dj;
        self.shared.dj.store(dj, Ordering::SeqCst);
        if !dj {
            self.console.leave();
            *self.shared.playback.lock().unwrap() = None;
            self.shared.state.store(
                if self.shared.paused.load(Ordering::SeqCst) {
                    STOPPED
                } else {
                    PLAYING
                },
                Ordering::SeqCst,
            );
        }
        self.publish();
        let _ = ack.send(Ok(()));
    }

    fn publish(&self) {
        if let Some(radio) = &self.radio {
            self.shared.has_stream.store(true, Ordering::SeqCst);
            self.shared
                .stream_start
                .store(radio.start.to_bits(), Ordering::Relaxed);
            self.shared.samples.store(radio.samples, Ordering::Relaxed);
            self.shared
                .stream_epoch
                .store(radio.epoch, Ordering::Relaxed);
        }
        if self.dj {
            let point = self.console.return_point();
            let playing = point.as_ref().is_some_and(|p| p.playing);
            *self.shared.playback.lock().unwrap() = point.map(|p| NowPlaying {
                track: p.track,
                elapsed_seconds: p.position,
                tempo: p.tempo,
            });
            self.shared
                .state
                .store(if playing { PLAYING } else { STOPPED }, Ordering::SeqCst);
        } else if let Some(radio) = &self.radio {
            if self.shared.state.load(Ordering::SeqCst) == super::LOADING {
                return;
            }
            if radio.ended {
                self.shared.state.store(super::ERROR, Ordering::SeqCst);
                *self.shared.message.lock().unwrap() = "the stream has ended".into();
            } else if self.shared.state.load(Ordering::SeqCst) != super::ERROR {
                self.shared.state.store(
                    if self.shared.paused.load(Ordering::SeqCst) {
                        STOPPED
                    } else {
                        PLAYING
                    },
                    Ordering::SeqCst,
                );
            }
        }
    }

    fn fill(&mut self) {
        self.controls();
        self.prepare_radio();
        self.block.clear();
        let volume = f32::from_bits(self.shared.volume.load(Ordering::Relaxed));
        for _ in 0..BLOCK {
            let fade = self.fade.as_ref().map(|f| {
                let w = 0.5 - 0.5 * (std::f32::consts::PI * f.at as f32 / (FADE - 1) as f32).cos();
                (f.to_dj, w)
            });
            for _ in 0..2 {
                let radio = if !self.dj || fade.is_some() {
                    if self.shared.paused.load(Ordering::SeqCst) {
                        0.0
                    } else {
                        self.radio
                            .as_mut()
                            .map_or(0.0, |r| r.heard(&self.shared) * volume)
                    }
                } else {
                    0.0
                };
                let console = if self.dj || fade.is_some() {
                    self.console.next().unwrap_or(0.0)
                } else {
                    0.0
                };
                let sample = match fade {
                    Some((true, w)) => radio * (1.0 - w) + console * w,
                    Some((false, w)) => console * (1.0 - w) + radio * w,
                    None => {
                        if self.dj {
                            console
                        } else {
                            radio
                        }
                    }
                };
                self.block.push(sample);
            }
            if let Some(fade) = &mut self.fade {
                fade.at += 1;
                if fade.at == FADE {
                    let fade = self.fade.take().unwrap();
                    self.finish(fade.to_dj, fade.ack);
                }
            }
        }
        let version = self.shared.eq_version.load(Ordering::Relaxed);
        if version != self.eq_seen {
            self.eq_seen = version;
            self.eq.set_gains(&self.shared.eq_gains.lock().unwrap());
        }
        self.eq.process(&mut self.block);
        self.publish();
    }
}

impl Iterator for Output {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.at >= self.block.len() {
            self.fill();
            self.at = 0;
        }
        let sample = self.block[self.at];
        self.at += 1;
        Some(sample)
    }
}

#[cfg(test)]
mod tests;

impl Source for Output {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        NonZero::new(2).unwrap()
    }
    fn sample_rate(&self) -> SampleRate {
        NonZero::new(RATE).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}
