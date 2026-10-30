//! The REST API, the streams and the spectrum over HTTP. Stage 4 of docs/rust-port.md, ported from the `_add_*_api`
//! functions, `_stream_handler`, `_add_spectrum_api` and `make_app` of `dj.py`.
//!
//! The answers (statuses, texts and JSON) are those of the Python radio, so the player and any script that talks to it
//! need not know which of them it talks to. No authentication: the radio is to be bound to trusted networks.

mod helpers;
mod streams;

pub use helpers::{ApiError, py_repr};

use crate::{
    backend::AuthError,
    encode::Streams,
    radio::{Radio, RadioError},
    spectrum::{SpectrumFeed, StartPositions},
};
use async_trait::async_trait;
use axum::{
    Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use helpers::{
    count, json_body, json_response, ok, parse_genres, parse_language, parse_moods, parse_move,
    parse_track_id, parse_track_ids,
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc};

/// The backends that can be turned on and off (`MultiBackend` in Python): the radio runs with several of them at once.
#[async_trait]
pub trait BackendSet: Send + Sync {
    /// Turn the backend on, and start signing in if it needs that. `Err` with the reason for an unknown one.
    async fn enable(&self, id: &str) -> Result<(), String>;
    /// Turn the backend off. `Err` with the reason for an unknown one.
    async fn disable(&self, id: &str) -> Result<(), String>;
    /// Describe every backend, for the page that turns them on and off.
    fn status(&self) -> Value;
    /// Change the settings of one of the backends that are on (not the sign-in).
    async fn update_backend_settings(
        &self,
        id: &str,
        values: &HashMap<String, String>,
    ) -> Result<(), AuthError>;
    /// Sign in to one of the backends that are on.
    async fn authorize_backend(
        &self,
        id: &str,
        values: &HashMap<String, String>,
    ) -> Result<(), AuthError>;
}

/// What the handlers work with.
#[derive(Clone)]
pub struct Api {
    pub radio: Arc<Radio>,
    /// `None`: the backends are not selectable.
    pub backends: Option<Arc<dyn BackendSet>>,
    pub streams: Streams,
    pub spectrum: Option<Arc<SpectrumFeed>>,
    pub starts: Arc<StartPositions>,
}

type Query_ = Query<Vec<(String, String)>>;

fn radio_error(error: RadioError) -> ApiError {
    match error {
        RadioError::UnknownQueueTrack(id) => {
            ApiError::not_found(format!("not in the queue: {}", py_repr(&id)))
        }
        RadioError::QueueChangeRefused(why) | RadioError::SkipRefused(why) => {
            ApiError::conflict(why)
        }
        RadioError::Invalid(why) => ApiError::bad_request(why),
        RadioError::Backend(why) => ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, why),
    }
}

impl Api {
    /// The radio's `auth_info`, with the backends if they are selectable.
    fn auth_info(&self) -> Value {
        let mut info = self.radio.auth_info();
        if let Some(backends) = &self.backends {
            info["backends"] = backends.status();
        }
        info
    }

    /// The moods and the language are settings of the account at the backend.
    fn need_token(&self) -> Result<(), ApiError> {
        if self.radio.authorized() {
            Ok(())
        } else {
            Err(ApiError::unavailable("not authorized yet"))
        }
    }

    fn steerable(&self) -> Result<(), ApiError> {
        self.need_token()?;
        if self.radio.language_info()["steered"] != true {
            return Err(ApiError::conflict("the backend can't change the language"));
        }
        Ok(())
    }

    fn queue(&self) -> Value {
        self.radio.queue(2, crate::radio::MAX_LISTED)
    }
}

// ---- The handlers ---------------------------------------------------------------------------------------------------

async fn root() -> Response {
    json_response(
        StatusCode::NOT_FOUND,
        &json!({"web": false, "stream": "/stream", "api": "/api/queue"}),
    )
}

async fn not_found(_uri: Uri) -> Response {
    ApiError::not_found("404: Not Found").into_response()
}

async fn not_allowed() -> Response {
    ApiError::new(StatusCode::METHOD_NOT_ALLOWED, "405: Method Not Allowed").into_response()
}

async fn queue(State(api): State<Api>, Query(query): Query_) -> Result<Response, ApiError> {
    let (previous, next) = (
        count(&query, "previous", 2)?,
        count(&query, "next", crate::radio::MAX_LISTED)?,
    );
    Ok(ok(&api.radio.queue(previous, next)))
}

async fn get_moods(State(api): State<Api>) -> Response {
    ok(&api.radio.moods_info())
}

async fn put_moods(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    api.need_token()?;
    let moods = parse_moods(&json_body(&body)?)?;
    api.radio
        .set_allowed_moods(moods)
        .await
        .map_err(radio_error)?;
    Ok(ok(&api.radio.moods_info()))
}

async fn delete_moods(State(api): State<Api>) -> Result<Response, ApiError> {
    api.need_token()?;
    api.radio
        .set_allowed_moods(Vec::new())
        .await
        .map_err(radio_error)?;
    Ok(ok(&api.radio.moods_info()))
}

async fn refresh_queue(State(api): State<Api>) -> Response {
    api.radio.refresh_queue();
    ok(&api.queue())
}

async fn clear_queue(State(api): State<Api>) -> Response {
    api.radio.clear_queue();
    ok(&api.queue())
}

async fn next_status(State(api): State<Api>) -> Response {
    ok(&api.radio.skip_info())
}

async fn next_track(State(api): State<Api>) -> Result<Response, ApiError> {
    api.radio.request_skip().map_err(radio_error)?;
    // 202: the blend starts when there is a good beat for it. The whole queue, like GET /api/queue gives it, not a part to
    // be taken for it.
    Ok(json_response(StatusCode::ACCEPTED, &api.queue()))
}

async fn pause(State(api): State<Api>) -> Response {
    api.radio.pause();
    ok(&api.radio.pause_info())
}

async fn resume(State(api): State<Api>) -> Response {
    api.radio.resume();
    ok(&api.radio.pause_info())
}

async fn playback(State(api): State<Api>, Query(query): Query_) -> Result<Response, ApiError> {
    let at = helpers::query_value(&query, "at")
        .map(|v| v.parse::<f64>())
        .transpose()
        .map_err(|_| ApiError::bad_request("invalid playback position"))?;
    Ok(ok(&json!(api.radio.playback(at).map_err(radio_error)?)))
}

async fn resume_playback(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    #[derive(serde::Deserialize)]
    struct Request {
        track_id: String,
        position: f64,
        next_track_id: Option<String>,
        #[serde(default)]
        paused: bool,
        #[serde(default)]
        advance: f64,
        lease_owner: Option<String>,
        #[serde(default)]
        heard: Vec<String>,
    }
    let request: Request = serde_json::from_value(json_body(&body)?)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    let applied = api
        .radio
        .resume_moving_playback(
            &request.track_id,
            request.position,
            request.next_track_id.as_deref(),
            request.advance,
            &request.heard,
        )
        .await
        .map_err(radio_error)?;
    let info = match tokio::time::timeout(std::time::Duration::from_secs(15), applied).await {
        Ok(Ok(info)) => info,
        result => {
            api.radio.cancel_playback_handoff();
            return Err(ApiError::unavailable(if result.is_err() {
                "the playback handoff timed out"
            } else {
                "the playback handoff ended"
            }));
        }
    };
    if request.paused {
        api.radio.pause();
    }
    if let Some(owner) = request.lease_owner {
        api.radio.release_playback_leases(&owner);
    }
    Ok(ok(&json!(info)))
}

async fn put_order(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    let ids = parse_track_ids(&json_body(&body)?)?;
    api.radio.queue_order(ids).map_err(radio_error)?;
    Ok(ok(&api.queue()))
}

async fn move_track(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    let (track_id, position) = parse_move(&json_body(&body)?)?;
    api.radio
        .queue_move(&track_id, position)
        .map_err(radio_error)?;
    Ok(ok(&api.queue()))
}

async fn remove_track(
    State(api): State<Api>,
    Path(track_id): Path<String>,
) -> Result<Response, ApiError> {
    api.radio.queue_remove(&track_id).map_err(radio_error)?;
    Ok(ok(&api.queue()))
}

async fn play_track(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    let track_id = parse_track_id(&json_body(&body)?)?;
    api.radio.queue_play(&track_id).map_err(radio_error)?;
    // the blend starts on a good beat
    Ok(json_response(StatusCode::ACCEPTED, &api.queue()))
}

/// The sound of a track of the queue as raw little-endian float32, stereo, 44.1 kHz (the decks of the player's console).
async fn track_pcm(
    State(api): State<Api>,
    Path(track_id): Path<String>,
    Query(query): Query_,
) -> Result<Response, ApiError> {
    let pcm = if let Some(owner) = helpers::query_value(&query, "dj_owner") {
        let slot = helpers::query_value(&query, "dj_slot")
            .and_then(|s| s.parse::<usize>().ok())
            .ok_or_else(|| ApiError::bad_request("invalid DJ deck slot"))?;
        let version = helpers::query_value(&query, "dj_version")
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or_else(|| ApiError::bad_request("invalid DJ deck version"))?;
        api.radio
            .leased_track_pcm(&track_id, owner, slot, version)
            .await
            .map_err(radio_error)?
    } else {
        api.radio.track_pcm(&track_id).await.map_err(radio_error)?
    };
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut response = bytes.into_response();
    response.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/octet-stream"),
    );
    Ok(response)
}

async fn get_mixing(State(api): State<Api>) -> Response {
    ok(&json!({"phrase": api.radio.phrase_mix()}))
}

/// `{"phrase": true}`: lay the blends by the phrases of the tracks (bar on bar, from where the outgoing one thins out).
async fn put_mixing(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    api.need_token()?;
    match json_body(&body)?.get("phrase") {
        Some(Value::Bool(on)) => api.radio.set_phrase_mix(*on),
        _ => return Err(ApiError::bad_request("expected {\"phrase\": true}")),
    }
    Ok(ok(&json!({"phrase": api.radio.phrase_mix()})))
}

async fn get_cycle(State(api): State<Api>) -> Response {
    ok(&api.radio.cycle_info())
}

/// `{"active": "off|morning|day|evening", "morning": {"stages": [BPM...], "stage_tracks": 3, "tolerance": 8},
/// "day": {"bpm_min": 95, "bpm_max": 135}, "evening": {...}}`, every part optional.
async fn put_cycle(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    let body = json_body(&body)?;
    api.radio.set_cycle(&body).await.map_err(radio_error)?;
    // the queue follows the plan
    api.radio.refresh_queue();
    Ok(ok(&api.radio.cycle_info()))
}

async fn get_arc(State(api): State<Api>) -> Response {
    ok(&api.radio.arc_info())
}

/// `{"enabled": bool, "stages": ["calm", ...], "stage_tracks": 3}`, every part optional (a plan needs both of its parts).
async fn put_arc(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    let body = json_body(&body)?;
    let enabled = match body.get("enabled") {
        None => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(ApiError::bad_request("\"enabled\" must be true or false")),
    };
    let plan = match (body.get("stages"), body.get("stage_tracks")) {
        (None, None) => None,
        (Some(stages), Some(tracks)) => {
            let stages = stages.as_array().and_then(|list| {
                list.iter()
                    .map(|v| v.as_str().map(str::to_owned))
                    .collect::<Option<Vec<_>>>()
            });
            let tracks = tracks.as_u64().map(|n| n as usize);
            match (stages, tracks) {
                (Some(stages), Some(tracks)) => Some((stages, tracks)),
                _ => {
                    return Err(ApiError::bad_request(
                        "expected {\"stages\": [...], \"stage_tracks\": 3}",
                    ));
                }
            }
        }
        _ => {
            return Err(ApiError::bad_request(
                "expected both \"stages\" and \"stage_tracks\"",
            ));
        }
    };
    api.radio
        .set_arc(enabled, plan)
        .await
        .map_err(radio_error)?;
    // the queue follows the arc
    api.radio.refresh_queue();
    Ok(ok(&api.radio.arc_info()))
}

async fn get_likes(State(api): State<Api>) -> Response {
    ok(&api.radio.likes_info().await)
}

async fn put_like(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    api.need_token()?;
    let body = json_body(&body)?;
    let track_id = parse_track_id(&body)?;
    let Some(liked) = body.get("liked").and_then(Value::as_bool) else {
        return Err(ApiError::bad_request(
            "expected {\"track_id\": \"...\", \"liked\": true}",
        ));
    };
    api.radio
        .set_like(&track_id, liked)
        .await
        .map_err(radio_error)?;
    Ok(ok(&api.radio.likes_info().await))
}

async fn get_language(State(api): State<Api>) -> Response {
    ok(&api.radio.language_info())
}

async fn put_language(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    api.steerable()?;
    let language = parse_language(&json_body(&body)?)?;
    api.radio
        .set_language(Some(&language))
        .await
        .map_err(radio_error)?;
    Ok(ok(&api.radio.language_info()))
}

async fn delete_language(State(api): State<Api>) -> Result<Response, ApiError> {
    api.steerable()?;
    api.radio.set_language(None).await.map_err(radio_error)?;
    Ok(ok(&api.radio.language_info()))
}

async fn get_genres(State(api): State<Api>) -> Response {
    ok(&api.radio.genres_info())
}

async fn put_genres(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    api.radio
        .set_wanted_genres(parse_genres(&json_body(&body)?)?);
    Ok(ok(&api.radio.genres_info()))
}

async fn delete_genres(State(api): State<Api>) -> Response {
    api.radio.set_wanted_genres(Vec::new());
    ok(&api.radio.genres_info())
}

async fn put_blocked(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    api.radio
        .set_blocked_genres(parse_genres(&json_body(&body)?)?);
    Ok(ok(&api.radio.genres_info()))
}

async fn delete_blocked(State(api): State<Api>) -> Response {
    api.radio.set_blocked_genres(Vec::new());
    ok(&api.radio.genres_info())
}

async fn stats(State(api): State<Api>) -> Response {
    ok(&api.radio.transition_stats())
}

async fn key_pairs(State(api): State<Api>) -> Response {
    ok(&api.radio.key_pair_stats())
}

async fn blends(State(api): State<Api>) -> Response {
    ok(&Value::Array(api.radio.recent_blends()))
}

async fn auth(State(api): State<Api>) -> Response {
    ok(&api.auth_info())
}

async fn sign_in(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    let body = json_body(&body)?;
    let which = body.get("backend").and_then(Value::as_str);
    let values: Option<HashMap<String, String>> =
        body.get("values").and_then(Value::as_object).and_then(|o| {
            o.iter()
                .map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        });
    let Some(values) = values else {
        return Err(ApiError::bad_request("expected {\"values\": {...}}"));
    };
    let signed = match (&api.backends, which) {
        (Some(backends), Some(which)) => backends.authorize_backend(which, &values).await,
        _ => api
            .radio
            .authorize(&values)
            .await
            .map_err(|e| AuthError(e.to_string())),
    };
    signed.map_err(|e| ApiError::bad_request(e.0))?;
    Ok(ok(&api.auth_info()))
}

fn known(api: &Api) -> Result<&Arc<dyn BackendSet>, ApiError> {
    api.backends
        .as_ref()
        .ok_or_else(|| ApiError::not_found("the backends are not selectable"))
}

async fn turn_on(State(api): State<Api>, body: Bytes) -> Result<Response, ApiError> {
    let body = json_body(&body)?;
    let Some(chosen) = body.get("backend").and_then(Value::as_str) else {
        return Err(ApiError::bad_request("expected {\"backend\": \"...\"}"));
    };
    known(&api)?
        .enable(chosen)
        .await
        .map_err(ApiError::bad_request)?;
    Ok(ok(&api.auth_info()))
}

/// `PUT /api/auth/backend/{backend}/settings` with `{"values": {...}}`: change the settings of a backend that is on.
async fn backend_settings(
    State(api): State<Api>,
    Path(backend): Path<String>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let body = json_body(&body)?;
    let values: Option<HashMap<String, String>> =
        body.get("values").and_then(Value::as_object).and_then(|o| {
            o.iter()
                .map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        });
    let Some(values) = values else {
        return Err(ApiError::bad_request("expected {\"values\": {...}}"));
    };
    known(&api)?
        .update_backend_settings(&backend, &values)
        .await
        .map_err(|e| ApiError::bad_request(e.0))?;
    // The tracks of another source are wanted: those waiting in the queue from this backend make way for them.
    if values.contains_key("tracks") {
        api.radio.drop_queued_of(&backend);
    }
    Ok(ok(&api.auth_info()))
}

async fn turn_off(
    State(api): State<Api>,
    Path(backend): Path<String>,
) -> Result<Response, ApiError> {
    known(&api)?
        .disable(&backend)
        .await
        .map_err(ApiError::not_found)?;
    api.radio.backends_changed();
    Ok(ok(&api.auth_info()))
}

/// The whole API, the streams and the spectrum as one router.
pub fn router(api: Api) -> Router {
    let mut app = Router::new()
        .route("/", get(root))
        .route("/api/queue", get(queue))
        .route(
            "/api/moods",
            get(get_moods).put(put_moods).delete(delete_moods),
        )
        .route("/api/queue/refresh", post(refresh_queue))
        .route("/api/queue/clear", post(clear_queue))
        .route("/api/next", get(next_status).post(next_track))
        .route("/api/pause", post(pause))
        .route("/api/resume", post(resume))
        .route("/api/playback", get(playback).post(resume_playback))
        .route("/api/queue/order", put(put_order))
        .route("/api/queue/move", post(move_track))
        .route(
            "/api/queue/tracks/{track_id}",
            axum::routing::delete(remove_track),
        )
        .route("/api/queue/play", post(play_track))
        .route(
            "/api/language",
            get(get_language).put(put_language).delete(delete_language),
        )
        .route(
            "/api/genres",
            get(get_genres).put(put_genres).delete(delete_genres),
        )
        .route(
            "/api/genres/blocked",
            put(put_blocked).delete(delete_blocked),
        )
        .route("/api/track/{track_id}/pcm", get(track_pcm))
        .route("/api/mixing", get(get_mixing).put(put_mixing))
        .route("/api/likes", get(get_likes).put(put_like))
        .route("/api/arc", get(get_arc).put(put_arc))
        .route("/api/cycle", get(get_cycle).put(put_cycle))
        .route("/api/stats", get(stats))
        .route("/api/transitions", get(blends))
        .route("/api/stats/keys", get(key_pairs))
        .route("/api/auth", get(auth).post(sign_in))
        .route("/api/auth/backend", put(turn_on))
        .route(
            "/api/auth/backend/{backend}",
            axum::routing::delete(turn_off),
        )
        .route(
            "/api/auth/backend/{backend}/settings",
            put(backend_settings),
        );
    app = streams::add_streams(app, &api);
    app.method_not_allowed_fallback(not_allowed)
        .fallback(not_found)
        .with_state(api)
}

/// Serve the API and the streams on a socket until `shutdown` is done.
pub async fn serve(
    listener: tokio::net::TcpListener,
    api: Api,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router(api))
        .with_graceful_shutdown(shutdown)
        .await
}
