//! The streams and the spectrum over HTTP. Task 4.4 of docs/rust-port.md, ported from `_stream_handler` and
//! `_add_spectrum_api` of `dj.py`.

use super::Api;
use crate::{
    spectrum::{SpectrumFeed, StartPositions, sse_frames, sse_start},
    stream::{
        broadcaster::Broadcaster,
        icy::{ICY_METAINT, ICY_NAME, IcyMux},
    },
};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::Response,
    routing::get,
};
use futures_util::stream;
use std::{convert::Infallible, sync::Arc, time::Duration};

/// How long the spectrum waits for the stream of a page to connect, how far before the start of it the frames begin, and
/// how often a quiet connection is asked if it is there.
const SPECTRUM_START_WAIT: Duration = Duration::from_secs(5);
const SPECTRUM_BACKLOG_MARGIN: f64 = 0.5;
const SPECTRUM_KEEPALIVE: Duration = Duration::from_secs(15);

fn query_name(query: &[(String, String)]) -> String {
    super::helpers::query_value(query, "l").unwrap_or("").to_owned()
}

/// Serve a hub to whoever asks. Listeners that ask for titles (`Icy-MetaData: 1`) get them, if the stream has them.
async fn stream_handler(hub: Arc<Broadcaster>, starts: Arc<StartPositions>, content_type: &'static str, titles: bool, headers: HeaderMap, query: Vec<(String, String)>) -> Response {
    let with_titles = titles && headers.get("icy-metadata").is_some_and(|v| v == "1");
    let (listener, position) = hub.subscribe_at();
    // The page says who it is in the address (?l=...) and asks for where in the mix its player starts, to show the spectrum
    // in time.
    let name = query_name(&query);
    if let (false, Some(position)) = (name.is_empty(), position) {
        starts.set(&name, position);
    }
    tracing::info!("Listener connected ({content_type})");
    let body = Body::from_stream(stream::unfold((listener, hub, IcyMux::default()), move |(listener, hub, mut mux)| async move {
        loop {
            let chunk = listener.recv().await;
            if !with_titles {
                return Some((Ok::<Bytes, Infallible>(chunk), (listener, hub, mux)));
            }
            let out = mux.push(&chunk, &hub.title());
            if !out.is_empty() {
                return Some((Ok(Bytes::from(out)), (listener, hub, mux)));
            }
        }
    }));
    let mut response = Response::new(body);
    let map = response.headers_mut();
    map.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    map.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    map.insert("icy-name", HeaderValue::from_static(ICY_NAME));
    // A page of another origin may analyze the audio: an <audio> with crossorigin hands it to Web Audio only if this is sent.
    map.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    if with_titles {
        map.insert("icy-metaint", HeaderValue::from_str(&ICY_METAINT.to_string()).expect("a number"));
    }
    response
}

/// `GET /api/spectrum`: the bands of the mix as server-sent events.
async fn spectrum_handler(feed: Arc<SpectrumFeed>, starts: Arc<StartPositions>, query: Vec<(String, String)>) -> Response {
    enum Phase {
        Start,
        Backlog(Vec<crate::spectrum::Frame>),
        Live,
    }
    struct State {
        feed: Arc<SpectrumFeed>,
        starts: Arc<StartPositions>,
        name: String,
        watcher: Option<crate::spectrum::Watcher>,
        phase: Phase,
    }
    let name = query_name(&query);
    let body = Body::from_stream(stream::unfold(State { feed, starts, name, watcher: None, phase: Phase::Start }, |mut state| async move {
        match std::mem::replace(&mut state.phase, Phase::Live) {
            Phase::Start => {
                let start = if state.name.is_empty() { None } else { state.starts.get(&state.name, SPECTRUM_START_WAIT).await };
                let (watcher, backlog) = state.feed.subscribe(start.map(|s| s - SPECTRUM_BACKLOG_MARGIN));
                state.watcher = Some(watcher);
                state.phase = Phase::Backlog(backlog);
                Some((Ok::<Bytes, Infallible>(Bytes::from(sse_start(start))), state))
            }
            Phase::Backlog(frames) if !frames.is_empty() => Some((Ok(Bytes::from(sse_frames(&frames))), state)),
            Phase::Backlog(_) | Phase::Live => {
                let watcher = state.watcher.as_ref().expect("subscribed");
                let text = match tokio::time::timeout(SPECTRUM_KEEPALIVE, watcher.recv()).await {
                    Ok(batch) => sse_frames(&batch),
                    Err(_) => ": keepalive\n\n".to_owned(),
                };
                Some((Ok(Bytes::from(text)), state))
            }
        }
    }));
    let mut response = Response::new(body);
    let map = response.headers_mut();
    map.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    map.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    map.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    // A proxy in front must not hold the events back.
    map.insert("x-accel-buffering", HeaderValue::from_static("no"));
    response
}

/// Add `/stream`, `/stream.flac` and `/stream.ogg` (the ones that are on) and `/api/spectrum`.
pub fn add_streams(mut app: Router<Api>, api: &Api) -> Router<Api> {
    let mut serve = |path: &'static str, hub: Arc<Broadcaster>, content_type: &'static str, titles: bool| {
        app = std::mem::take(&mut app).route(
            path,
            get(move |State(api): State<Api>, headers: HeaderMap, Query(query): Query<Vec<(String, String)>>| {
                let hub = hub.clone();
                async move { stream_handler(hub, api.starts.clone(), content_type, titles, headers, query).await }
            }),
        );
    };
    serve("/stream", api.streams.mp3.clone(), "audio/mpeg", true);
    if let Some(hub) = &api.streams.flac {
        serve("/stream.flac", hub.clone(), "audio/flac", false);
    }
    if let Some(hub) = &api.streams.ogg {
        serve("/stream.ogg", hub.clone(), "audio/ogg", false);
    }
    if let Some(feed) = &api.spectrum {
        let feed = feed.clone();
        app = app.route(
            "/api/spectrum",
            get(move |State(api): State<Api>, Query(query): Query<Vec<(String, String)>>| {
                let feed = feed.clone();
                async move { spectrum_handler(feed, api.starts.clone(), query).await }
            }),
        );
    }
    app
}
