//! The mix as a series of pieces: the decks (decoded tracks), where a blend goes, and how the pieces are cut. Tasks 3.2.1
//! onwards of docs/rust-port-radio.md, ported from `dj.py`: `Deck`, `BlendPlan`, `_pieces`, `_plan`, `_early_plan`,
//! `_beat_period`, `_followed_until`.

use crate::{
    backend::Track,
    mix::{CHANNELS, SAMPLE_RATE, TrackAnalysis, glide::NEGLIGIBLE_TEMPO_SHIFT, plan},
};
use std::sync::Arc;

/// The mix is produced in pieces of this length.
pub const CHUNK_SECONDS: f64 = 0.25;
/// The next track is chosen and decoded this long before the blend.
pub const LOOKAHEAD_SECONDS: f64 = 40.0;
/// A beat grid of a blend needs at least this many points to give a period.
const MIN_BEATS_IN_GRID: usize = 2;
/// A blend asked for at once (the skip button) is let go with less room than this.
const MIN_EARLY_BLEND_SECONDS: f64 = plan::MIN_EARLY_BLEND_SECONDS;

/// A piece of the mix, and the title of what is heard in it.
#[derive(Debug, Clone, PartialEq)]
pub struct Piece {
    /// Stereo, interleaved.
    pub pcm: Vec<f32>,
    pub title: Arc<str>,
}

/// A decoded track as it will be played.
#[derive(Debug, Clone)]
pub struct Deck {
    pub title: Arc<str>,
    /// Stereo, interleaved, 44.1 kHz.
    pub pcm: Arc<[f32]>,
    pub analysis: TrackAnalysis,
    pub beat_matched: bool,
    pub track: Option<Track>,
    pub mood: String,
    /// The ratio of the tempo of the track before it to its own tempo (a beat-matched blend glides through it, see
    /// `glide::tempo_glide`).
    pub speed: f64,
    /// `Radio::generation` when the deck was chosen.
    pub generation: u64,
    /// The downloaded track it was made from, kept until the deck is heard so that the deck can go back to the pool.
    pub source: Option<crate::radio::Candidate>,
}

impl Deck {
    /// How many frames of sound the deck holds.
    pub fn frames(&self) -> i64 {
        (self.pcm.len() / CHANNELS) as i64
    }
}

/// Where a blend takes its pieces from (samples), and at what pace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlendPlan {
    /// In the outgoing track.
    pub start: i64,
    /// In the incoming track.
    pub following_start: i64,
    /// Of the outgoing piece.
    pub length: i64,
    /// Incoming samples per outgoing sample.
    pub speed: f64,
}

/// Cut a track into pieces of a quarter of a second, the last one shorter. The pieces are made one at a time.
pub fn pieces<'a>(pcm: &'a [f32], title: &'a Arc<str>) -> impl Iterator<Item = Piece> + 'a {
    let size = (CHUNK_SECONDS * f64::from(SAMPLE_RATE)).round_ties_even() as usize * CHANNELS;
    pcm.chunks(size).map(move |chunk| Piece { pcm: chunk.to_vec(), title: Arc::clone(title) })
}

/// Samples of a stretch of the following deck from `start` at `speed`: the Python `int(frames / speed)`.
fn playable(following: &Deck, following_start: i64, speed: f64) -> i64 {
    ((following.frames() - following_start) as f64 / speed) as i64
}

/// Plan the blend of the following deck into the current one. A deck that has no room for a blend on the beat is not
/// beat-matched any more (`following.beat_matched` says so).
pub fn plan_blend(current: &Deck, following: &mut Deck, position: i64) -> BlendPlan {
    let matched = if following.beat_matched {
        plan::plan_matched(&current.analysis, &following.analysis, following.speed)
    } else {
        None
    };
    // Without room for a blend on the beat the tracks are not matched.
    following.beat_matched = matched.is_some();
    let planned = matched.unwrap_or_else(|| plan::plan_blend(&current.analysis, &following.analysis, false, 1.0));
    let speed = if following.beat_matched { following.speed } else { 1.0 };
    let start = planned.start.max(position);
    let length = 0.max(
        planned
            .length
            .min(current.analysis.end - start)
            // The decoded track may be a few samples shorter than its analysis says (rounding).
            .min(current.frames() - start)
            .min(playable(following, planned.following_start, speed)),
    );
    BlendPlan { start, following_start: planned.following_start, length, speed }
}

/// Plan the blend asked for by the skip button, if it can be made now. It needs some room left in both tracks. (The
/// caller has seen that the following deck is ready and not stale.)
pub fn plan_early(current: &Deck, following: &Deck, position: i64) -> Option<BlendPlan> {
    let speed = if following.beat_matched { following.speed } else { 1.0 };
    let planned = plan::plan_early(&current.analysis, &following.analysis, position, following.beat_matched, speed)?;
    let length = planned.length.min(current.frames() - planned.start).min(playable(following, planned.following_start, speed));
    if (length as f64) < MIN_EARLY_BLEND_SECONDS * f64::from(SAMPLE_RATE) {
        return None;
    }
    Some(BlendPlan { start: planned.start, following_start: planned.following_start, length, speed })
}

/// The samples per beat of the blend, 0 if the beats don't match.
pub fn beat_period(current: &Deck, following: &Deck, beat_grid: Option<&[f64]>) -> f64 {
    if !following.beat_matched {
        return 0.0;
    }
    match beat_grid {
        Some(grid) if grid.len() >= MIN_BEATS_IN_GRID => (grid[grid.len() - 1] - grid[0]) / (grid.len() - 1) as f64,
        _ => current.analysis.beat_period(),
    }
}

/// Where in the following deck its own tempo goes on after a blend. A blend that glided takes `speed` times as much of
/// the following track as of the outgoing one, however long the glide made the blend.
pub fn followed_until(plan: &BlendPlan, following: &Deck) -> i64 {
    let glided = following.beat_matched && (plan.speed - 1.0).abs() >= NEGLIGIBLE_TEMPO_SHIFT;
    plan.following_start + (plan.length as f64 * if glided { plan.speed } else { 1.0 }).round_ties_even() as i64
}

// ---- The blend itself (tasks 3.2.2 and 3.2.3) ----------------------------------------------------------------------
// Ported from `dj.py`: `BlendReport`, `DeckSource`, `_blend_pieces`, `_transition`.

use crate::{
    clock::Clock,
    mix::{
        blend, effects::{self, Effect, NoiseSource}, glide, key,
        plan::{self as mix_plan, Transition},
    },
};
use async_trait::async_trait;
use serde::Serialize;
use std::sync::Mutex;
use tokio::sync::mpsc::Sender;

/// Python's `round(value, digits)`: the decimal number nearest to the value.
pub fn round_to(value: f64, digits: usize) -> f64 {
    format!("{value:.digits$}").parse().unwrap_or(value)
}

/// How one blend was made, to look at when it did not sound right.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BlendReport {
    #[serde(rename = "from")]
    pub from_title: String,
    #[serde(rename = "to")]
    pub to_title: String,
    pub kind: String,
    pub effect: String,
    pub beat_matched: bool,
    pub from_bpm: f64,
    pub to_bpm: f64,
    /// How far the tempo glided, as a fraction (0.05: 5% faster); 0 if not.
    pub glide: f64,
    pub seconds: f64,
    pub loudness_gap_db: f64,
    pub key_relation: String,
    /// How far above the ceiling the blend peaked before it was limited.
    pub over_db: f64,
    pub at: f64,
}

/// The radio could not make a deck.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeckError(pub String);

impl std::fmt::Display for DeckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for DeckError {}

/// What the mix needs of the radio: decks to play, and word of them.
#[async_trait]
pub trait DeckSource: Send + Sync {
    /// The next track was asked for (the skip button).
    fn skip_requested(&self) -> bool;
    /// Choose, tempo-match and decode the track to play after `current`.
    async fn next_deck(&self, current: Option<&TrackAnalysis>) -> Result<Deck, DeckError>;
    /// Whether the queue was rebuilt after the deck was chosen.
    fn is_stale(&self, deck: &Deck) -> bool;
    /// Note that the track that plays is left early, at the user's wish.
    fn skip_taken(&self);
    /// Count a blend of the current deck into the following one.
    fn transition(&self, kind: Transition, current: &Deck, following: &Deck, effect: Effect, report: Option<BlendReport>);
    /// Tell that the deck has begun to be heard.
    fn started(&self, deck: &Deck);
    /// Tell that the deck has been heard to the end, or skipped.
    fn finished(&self, deck: &Deck);
    /// Paused through the API.
    fn user_paused(&self) -> bool;
    /// Note that the mix waits for listeners, or goes on.
    fn hold(&self, held: bool);
    /// Return once the mix is not paused.
    async fn wait_resumed(&self);
}

/// The noise of the effects, shared with the thread that makes the blend.
pub type SharedNoise = std::sync::Arc<Mutex<dyn NoiseSource + Send>>;

/// The consumer of the mix has gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

/// Frames `from..to` of a piece, as Python's slice: it stops at the ends.
fn slice(pcm: &[f32], from: i64, to: i64) -> &[f32] {
    let frames = (pcm.len() / CHANNELS) as i64;
    let (from, to) = (from.clamp(0, frames) as usize, to.clamp(0, frames) as usize);
    &pcm[from * CHANNELS..to.max(from) * CHANNELS]
}

/// Cut the two pieces that are blended, equally long. If the tempos differ, the pieces glide from one to the other (and
/// the beats of the blend are returned); otherwise they are just cut out.
pub async fn blend_pieces(current: &Deck, following: &mut Deck, plan: &BlendPlan) -> (Vec<f32>, Vec<f32>, Option<Vec<f64>>) {
    let (start, following_start) = (plan.start, plan.following_start);
    let glides = following.beat_matched && (plan.speed - 1.0).abs() >= glide::NEGLIGIBLE_TEMPO_SHIFT;
    if glides {
        let span = (plan.length as f64 * plan.speed).round_ties_even() as i64;
        let margin = glide::GLIDE_MARGIN as i64;
        let outgoing = slice(&current.pcm, start, start + plan.length + margin).to_vec();
        let incoming = slice(&following.pcm, following_start, following_start + span + margin).to_vec();
        let (bpm, speed, length) = (current.analysis.bpm, plan.speed, plan.length as usize);
        // Stretching takes a while too.
        match tokio::task::spawn_blocking(move || glide::tempo_glide(&outgoing, &incoming, bpm, speed, length)).await {
            Ok((outgoing, incoming, grid)) => return (outgoing, incoming, Some(grid)),
            Err(error) => {
                tracing::error!("The tempo glide failed, the beats will not match: {error}");
                following.beat_matched = false;
            }
        }
    }
    let outgoing = slice(&current.pcm, start, start + plan.length);
    let incoming = slice(&following.pcm, following_start, following_start + plan.length);
    // The pieces must be equally long, whatever the analysis says.
    let length = outgoing.len().min(incoming.len());
    (outgoing[..length].to_vec(), incoming[..length].to_vec(), None)
}

/// Where the pieces of the mix go. In the radio it is a channel to the encoders; a test takes them one by one and
/// changes the radio between two, as the consumer of the generator of the Python radio does.
#[async_trait]
pub trait PieceSink: Send + Sync {
    /// Take a piece; `Err(Closed)` when nobody wants more.
    async fn send(&self, piece: Piece) -> Result<(), Closed>;
}

#[async_trait]
impl PieceSink for Sender<Piece> {
    async fn send(&self, piece: Piece) -> Result<(), Closed> {
        Sender::send(self, piece).await.map_err(|_| Closed)
    }
}

async fn emit(out: &dyn PieceSink, piece: Piece) -> Result<(), Closed> {
    out.send(piece).await
}

/// Blend the following deck into the current one, piece by piece. The pieces go to `out`; `Err(Closed)` if nobody takes
/// them any more.
#[allow(clippy::too_many_arguments)]
pub async fn transition(
    radio: &dyn DeckSource,
    clock: &dyn Clock,
    current: &Deck,
    following: &mut Deck,
    plan: &BlendPlan,
    effects_level: f64,
    noise: SharedNoise,
    out: &dyn PieceSink,
) -> Result<(), Closed> {
    let (outgoing, incoming, beat_grid) = blend_pieces(current, following, plan).await;
    let frames = outgoing.len() / CHANNELS;
    let period = beat_period(current, following, beat_grid.as_deref());
    let seconds = frames as f64 / f64::from(SAMPLE_RATE);
    let kind = mix_plan::choose_transition(&current.analysis, &following.analysis, following.beat_matched, seconds);
    let mut used = effects::applied_blend(kind, frames, period, effects_level);
    // The filtering takes a while, don't stall the event loop with it.
    let task = {
        let (outgoing, incoming, grid, noise) = (outgoing.clone(), incoming.clone(), beat_grid.clone(), noise);
        tokio::task::spawn_blocking(move || {
            let mut noise = noise.lock().expect("not poisoned");
            effects::blend(kind, &outgoing, &incoming, period, effects_level, &mut *noise, grid.as_deref())
        })
    };
    let mut blended = match task.await {
        Ok(blended) => blended,
        Err(error) => {
            // A broken blend must not end the stream: fade instead.
            tracing::error!("Blend failed ({}), crossfading: {error}", kind.name());
            used = effects::Applied { kind: Transition::Crossfade, effect: Effect::None };
            blend::crossfade(&outgoing, &incoming)
        }
    };
    // Two loud tracks and the effects add up to more than fits in the encoder.
    let peak = f64::from(blended.iter().fold(0.0f32, |top, x| top.max(x.abs())));
    let over_db = if peak != 0.0 { (20.0 * (peak / 0.97).log10()).max(0.0) } else { 0.0 };
    blended = effects::limit(&blended, effects::LIMIT_CEILING);
    let blended_frames = blended.len() / CHANNELS;
    let report = BlendReport {
        from_title: current.title.to_string(),
        to_title: following.title.to_string(),
        kind: used.kind.name().to_owned(),
        effect: used.effect.name().to_owned(),
        beat_matched: following.beat_matched,
        from_bpm: round_to(current.analysis.bpm, 1),
        to_bpm: round_to(following.analysis.bpm, 1),
        glide: if beat_grid.is_some() { round_to(plan.speed - 1.0, 3) } else { 0.0 },
        seconds: round_to(blended_frames as f64 / f64::from(SAMPLE_RATE), 1),
        loudness_gap_db: round_to((current.analysis.loudness_db - following.analysis.loudness_db).abs(), 1),
        key_relation: key::key_relation(current.analysis.key, following.analysis.key).name().to_owned(),
        over_db: round_to(over_db, 1),
        at: clock.wall(),
    };
    tracing::info!(
        "Transition: {}, effect {}, {:.1} s, glide {:+.1}%, over the ceiling by {:.1} dB",
        used.kind.name(),
        used.effect.name(),
        report.seconds,
        report.glide * 100.0,
        report.over_db
    );
    // Count what was really made, not what was meant.
    radio.transition(used.kind, current, following, used.effect, Some(report));
    radio.started(following);
    let half = blended_frames / 2;
    for piece in pieces(&blended[..half * CHANNELS], &current.title) {
        emit(out, piece).await?;
    }
    for piece in pieces(&blended[half * CHANNELS..], &following.title) {
        emit(out, piece).await?;
    }
    radio.finished(current);
    Ok(())
}

// ---- The endless mix (tasks 3.2.4 and 3.2.5) ------------------------------------------------------------------------
// Ported from `dj.py`: `_ready_deck`, `_fresh`, `_early_plan` (the part that looks at the radio), `_approach`, `mix`.

use tokio::task::JoinHandle;

/// Why the mix ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MixError {
    /// The radio could not make a deck.
    Deck(DeckError),
    /// Nobody takes the pieces any more.
    Closed,
}

impl From<DeckError> for MixError {
    fn from(error: DeckError) -> Self {
        Self::Deck(error)
    }
}

impl From<Closed> for MixError {
    fn from(_: Closed) -> Self {
        Self::Closed
    }
}

impl std::fmt::Display for MixError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deck(error) => error.fmt(f),
            Self::Closed => f.write_str("the mix is not taken any more"),
        }
    }
}

impl std::error::Error for MixError {}

/// A deck that is being made, in a task of its own. It can be looked at before it is done (the Python radio asks a
/// task for its result); a task that is let go is stopped.
struct NextDeck {
    slot: Arc<Mutex<Option<Result<Deck, DeckError>>>>,
    handle: JoinHandle<()>,
}

impl NextDeck {
    fn spawn(radio: &Arc<dyn DeckSource>, current: &TrackAnalysis) -> Self {
        let slot = Arc::new(Mutex::new(None));
        let (radio, current, put) = (Arc::clone(radio), current.clone(), Arc::clone(&slot));
        let handle = tokio::spawn(async move {
            let result = radio.next_deck(Some(&current)).await;
            *put.lock().expect("not poisoned") = Some(result);
        });
        Self { slot, handle }
    }

    /// The deck, if the task is done and made one. (A deck is cheap to copy: its samples are shared.)
    fn ready(&self) -> Option<Deck> {
        match self.slot.lock().expect("not poisoned").as_ref() {
            Some(Ok(deck)) => Some(deck.clone()),
            _ => None,
        }
    }

    /// Wait for the deck.
    async fn wait(&mut self) -> Result<Deck, DeckError> {
        let finished = (&mut self.handle).await;
        match self.slot.lock().expect("not poisoned").take() {
            Some(result) => result,
            None => Err(DeckError(format!("the task that makes the next deck ended: {finished:?}"))),
        }
    }
}

impl Drop for NextDeck {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Wait for the next deck; choose again if the queue was rebuilt.
async fn ready_deck(radio: &Arc<dyn DeckSource>, current: &TrackAnalysis, task: &mut NextDeck) -> Result<Deck, DeckError> {
    let mut following = task.wait().await?;
    while radio.is_stale(&following) {
        following = radio.next_deck(Some(current)).await?;
    }
    Ok(following)
}

/// Choose the next deck anew at once if the queue was changed. A deck that was chosen before the change would wait to be
/// found stale until the track is nearly over; a track asked for would wait with it.
fn fresh(radio: &Arc<dyn DeckSource>, current: &Deck, task: NextDeck) -> NextDeck {
    match task.ready() {
        Some(deck) if radio.is_stale(&deck) => NextDeck::spawn(radio, &current.analysis),
        _ => task,
    }
}

/// Plan the blend asked for by the skip button, if it can be made now. It needs the next deck to be ready (the music
/// goes on until it is) and some room left in both tracks.
fn early_plan(radio: &Arc<dyn DeckSource>, current: &Deck, task: &NextDeck, position: i64) -> Option<BlendPlan> {
    let following = task.ready()?;
    if radio.is_stale(&following) {
        return None;
    }
    plan_early(current, &following, position)
}

/// Play the track up to its blend, and choose again if the queue changed. Until the blend begins the queue may still be
/// changed by hand: the deck that was chosen is then given up for another, and the blend planned anew.
async fn approach(
    radio: &Arc<dyn DeckSource>,
    current: &Deck,
    following: &mut Deck,
    plan: &mut BlendPlan,
    position: i64,
    out: &dyn PieceSink,
) -> Result<(), MixError> {
    let mut position = position;
    loop {
        let mut stale = false;
        for piece in pieces(slice(&current.pcm, position, plan.start), &current.title) {
            let frames = (piece.pcm.len() / CHANNELS) as i64;
            emit(out, piece).await?;
            position += frames;
            if radio.is_stale(following) {
                stale = true;
                break;
            }
        }
        if !stale {
            return Ok(());
        }
        let mut task = NextDeck::spawn(radio, &current.analysis);
        *following = ready_deck(radio, &current.analysis, &mut task).await?;
        *plan = plan_blend(current, following, position);
    }
}

/// Produce the endless mix: PCM pieces and the title of each. It ends when a deck cannot be made or when nobody takes
/// the pieces; the tasks that make decks are stopped with it.
pub async fn mix(
    radio: Arc<dyn DeckSource>,
    clock: Arc<dyn Clock>,
    effects_level: f64,
    noise: SharedNoise,
    out: &dyn PieceSink,
) -> Result<(), MixError> {
    let mut current = radio.next_deck(None).await?;
    radio.started(&current);
    let mut position = current.analysis.start;
    let lookahead = (LOOKAHEAD_SECONDS * f64::from(SAMPLE_RATE)) as i64;

    loop {
        let mut task = NextDeck::spawn(&radio, &current.analysis);

        // Everything up to the look-ahead point can be played without knowing the next track, unless the next track is
        // asked for.
        let solo_end = position.max(current.analysis.end - lookahead);
        let mut early = None;
        for piece in pieces(slice(&current.pcm, position, solo_end), &current.title) {
            let frames = (piece.pcm.len() / CHANNELS) as i64;
            emit(out, piece).await?;
            position += frames;
            task = fresh(&radio, &current, task);
            if radio.skip_requested() {
                early = early_plan(&radio, &current, &task, position);
                if early.is_some() {
                    break;
                }
            }
        }
        if early.is_none() {
            position = solo_end;
        }

        let mut following = ready_deck(&radio, &current.analysis, &mut task).await?;
        if early.is_some() {
            radio.skip_taken();
        }
        let mut plan = match early {
            Some(plan) => plan,
            None => plan_blend(&current, &mut following, position),
        };

        approach(&radio, &current, &mut following, &mut plan, position, out).await?;
        transition(&*radio, &*clock, &current, &mut following, &plan, effects_level, Arc::clone(&noise), out).await?;
        position = followed_until(&plan, &following);
        current = following;
    }
}
