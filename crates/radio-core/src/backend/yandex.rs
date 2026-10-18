//! The Yandex Music backend. Task 5.3 of docs/rust-port.md, ported from `ya_player.py` and `request.py`, and from the parts
//! of the `yandex_music` library that they use: the OAuth device flow, the station ("rotor") calls, the signed request for
//! the link to a file.
//!
//! Signing in is a code on Yandex's page: the radio asks for the code, shows it (`auth_prompt`), and polls until it is
//! entered; the token is kept in `~/.config/yamusic-radio.json`, for the owner only.

use super::{AuthError, AuthPrompt, BackendError, Feedback, MusicBackend, Track};
use crate::clock::Clock;
use async_trait::async_trait;
use base64::Engine as _;
use hmac::{Hmac, KeyInit, Mac};
use reqwest::Method;
use serde_json::{Map, Value, json};
use sha2::Sha256;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    sync::{Notify, Semaphore},
    task::JoinHandle,
    time::Instant,
};

pub const API: &str = "https://api.music.yandex.net";
pub const OAUTH: &str = "https://oauth.yandex.ru";
/// "My wave".
pub const DEFAULT_STATION: &str = "user:onyourwave";
const CLIENT_HEADER: &str = "YandexMusicAndroid/24023621";
const USER_AGENT: &str = "Yandex-Music-API";
/// The public OAuth credentials of the official Android app.
const CLIENT_ID: &str = "23cabbbdc6cd418abb4b39c32c41195d";
const CLIENT_SECRET: &str = "53bc75238f0c4d08a118e51fe9203300";
const DEVICE_NAME: &str = "YandexMusicAPI";
/// The key of the Android app, which signs the request for a link to a file.
const SIGN_KEY: &str = "p93jhgh689SBReK6ghtw62";
/// "lossless" is served as FLAC (and AAC) in an MP4 container: the plain flac and aac codecs are not offered for it, so the
/// *-mp4 ones must be requested.
const CODECS: &str = "flac,aac,he-aac,mp3";
const LOSSLESS_CODECS: &str = "flac,flac-mp4,aac,aac-mp4,he-aac,he-aac-mp4,mp3";
/// Yandex answers 429 above a few simultaneous get-file-info requests; callers share these slots, and the pause after a 429.
const DOWNLOAD_INFO_SLOTS: usize = 3;
/// How long a request that Yandex limited waits before it is sent again, one after the other.
pub const RATE_LIMIT_DELAYS: [f64; 4] = [0.5, 1.0, 2.0, 4.0];
const RETRY_AUTH_SECONDS: u64 = 30;

/// Where the service is, and how long to wait when it limits the requests (the tests change these).
#[derive(Debug, Clone)]
pub struct YandexOptions {
    pub api: String,
    pub oauth: String,
    pub rate_limit_delays: Vec<f64>,
}

impl Default for YandexOptions {
    fn default() -> Self {
        Self { api: API.to_owned(), oauth: OAUTH.to_owned(), rate_limit_delays: RATE_LIMIT_DELAYS.to_vec() }
    }
}

// ---- The settings file --------------------------------------------------------------------------------------------------

/// `~/.config/yamusic-radio.json`
pub fn default_settings_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/yamusic-radio.json"))
}

/// Whether the settings file already holds an OAuth token.
pub fn has_token(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| v.get("token").cloned())
        .is_some_and(|t| t.as_str().is_some_and(|s| !s.is_empty()) || (t.is_boolean() && t == true) || t.is_number())
}

fn read_settings(path: &Path) -> std::io::Result<Map<String, Value>> {
    let mut settings = Map::new();
    settings.insert("token".to_owned(), Value::Null);
    settings.insert("from_id".to_owned(), json!(format!("music-{}", uuid::Uuid::new_v4())));
    settings.insert("quality".to_owned(), json!("lossless"));
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let Value::Object(saved) = serde_json::from_str(&text).map_err(std::io::Error::other)? else {
                return Err(std::io::Error::other("the settings file is not an object"));
            };
            settings.extend(saved);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => save_settings(path, &settings)?,
        Err(error) => return Err(error),
    }
    Ok(settings)
}

/// Write the settings, which hold the token, for the owner only.
fn save_settings(path: &Path, settings: &Map<String, Value>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A new file, so that the token is never readable by others, not even for a moment, and a crash can't leave half of it
    // behind.
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    std::io::Write::write_all(&mut options.open(&temp)?, serde_json::to_string(settings).expect("settings are JSON").as_bytes())?;
    std::fs::rename(temp, path)
}

// ---- The requests -----------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    BadRequest,
    RateLimit,
    Other,
}

/// What the service did wrong; the text is what Python's `str(error)` says.
#[derive(Debug, Clone)]
struct YError {
    kind: Kind,
    message: String,
}

impl YError {
    fn other(message: impl Into<String>) -> Self {
        Self { kind: Kind::Other, message: message.into() }
    }
}

impl std::fmt::Display for YError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<YError> for BackendError {
    fn from(error: YError) -> Self {
        BackendError(error.message)
    }
}

/// Python's `repr()` of bytes: `b'...'`.
fn bytes_repr(content: &[u8]) -> String {
    let quote = if content.contains(&b'\'') && !content.contains(&b'"') { b'"' } else { b'\'' };
    let mut out = String::from("b");
    out.push(quote as char);
    for &b in content {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            b if b == quote => {
                out.push('\\');
                out.push(b as char);
            }
            0x20..=0x7e => out.push(b as char),
            b => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push(quote as char);
    out
}

/// A response of the API: `result` if there is one, else the whole of it.
struct Response {
    data: Value,
    result: Option<Value>,
    error: Option<String>,
    error_description: Option<String>,
}

impl Response {
    fn get_error(&self) -> String {
        format!("{} {}", self.error.as_deref().unwrap_or("None"), self.error_description.as_deref().unwrap_or(""))
    }

    fn get_result(self) -> Value {
        self.result.unwrap_or(self.data)
    }
}

/// `_parse` of the library: a body that is not JSON is an error; one that is not an object has no response.
fn parse(content: &[u8]) -> Result<Option<Response>, YError> {
    let data: Value = serde_json::from_slice(content).map_err(|_| YError::other("Invalid server response"))?;
    let Value::Object(object) = &data else { return Ok(None) };
    let text = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_owned);
    match object.get("result") {
        Some(result) if !result.is_null() => Ok(Some(Response { result: Some(result.clone()), error: text("error"), error_description: text("errorDescription"), data })),
        // The data are in the root: the response is made of them.
        _ => Ok(Some(Response { result: Some(data.clone()), error: text("error"), error_description: text("errorDescription"), data })),
    }
}

/// The address without its query: it holds signatures.
fn where_(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or(url).to_owned()
}

/// A pause the requests share: when Yandex limits one, all of them wait. Each request backing off on its own sends the next
/// burst at the same moment, which is limited again.
#[derive(Default)]
struct Cooldown {
    until: Mutex<Option<Instant>>,
}

impl Cooldown {
    async fn wait(&self) {
        let until = *self.until.lock().expect("not poisoned");
        if let Some(until) = until {
            tokio::time::sleep_until(until).await;
        }
    }

    /// Pause for `delay`, unless a pause is on already; say if it began.
    fn start(&self, delay: Duration) -> bool {
        let now = Instant::now();
        let mut until = self.until.lock().expect("not poisoned");
        if until.is_some_and(|u| u > now) {
            return false;
        }
        *until = Some(now + delay);
        true
    }
}

enum Body {
    None,
    Form(Vec<(&'static str, String)>),
    Json(Value),
}

struct State {
    settings: Map<String, Value>,
    auth_url: Option<String>,
    user_code: Option<String>,
    /// The mood this process set the station to, `None` before that.
    mood: Option<String>,
    last_track: Option<String>,
    last_station: (String, String),
    init_task: Option<JoinHandle<()>>,
    signed_in: bool,
}

/// The Yandex Music implementation of [`MusicBackend`].
pub struct YandexBackend {
    me: Weak<Self>,
    http: reqwest::Client,
    options: YandexOptions,
    settings_path: PathBuf,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    signed_in: Notify,
    info_slots: Semaphore,
    cooldown: Cooldown,
}

impl YandexBackend {
    pub fn new(settings_path: PathBuf, options: YandexOptions, clock: Arc<dyn Clock>) -> std::io::Result<Arc<Self>> {
        let settings = read_settings(&settings_path)?;
        let signed_in = settings.get("token").and_then(Value::as_str).is_some_and(|t| !t.is_empty());
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(10))
            .build()
            .map_err(std::io::Error::other)?;
        Ok(Arc::new_cyclic(|me| Self {
            me: me.clone(),
            http,
            options,
            settings_path,
            clock,
            state: Mutex::new(State {
                settings,
                auth_url: None,
                user_code: None,
                mood: None,
                last_track: None,
                last_station: (String::new(), String::new()),
                init_task: None,
                signed_in,
            }),
            signed_in: Notify::new(),
            info_slots: Semaphore::new(DOWNLOAD_INFO_SLOTS),
            cooldown: Cooldown::default(),
        }))
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("not poisoned")
    }

    fn token(&self) -> Option<String> {
        self.st().settings.get("token").and_then(Value::as_str).map(str::to_owned)
    }

    fn setting(&self, key: &str) -> String {
        self.st().settings.get(key).and_then(Value::as_str).unwrap_or("").to_owned()
    }

    fn from_id(&self) -> String {
        self.setting("from_id")
    }

    /// One request of the API: what comes back is the `result` of it.
    async fn request(&self, method: Method, url: &str, params: &[(&str, String)], body: Body) -> Result<Option<Value>, YError> {
        let mut request = self.http.request(method.clone(), url).header("X-Yandex-Music-Client", CLIENT_HEADER).header("User-Agent", USER_AGENT);
        if let Some(token) = self.token().filter(|t| !t.is_empty()) {
            request = request.header("Authorization", format!("OAuth {token}"));
        }
        if !params.is_empty() {
            request = request.query(params);
        }
        request = match body {
            Body::None => request,
            Body::Form(form) => request.form(&form),
            Body::Json(json) => request.header("Content-Type", "application/json").body(serde_json::to_vec(&json).expect("a value is JSON")),
        };
        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                YError::other("Timed out")
            } else {
                YError::other(format!("{method} {}: {e}", where_(url)))
            }
        })?;
        let status = response.status().as_u16();
        let content = response.bytes().await.map_err(|e| if e.is_timeout() { YError::other("Timed out") } else { YError::other(format!("{method} {}: {e}", where_(url))) })?;
        if status < 400 {
            return Ok(parse(&content)?.map(Response::get_result));
        }
        let message = match parse(&content) {
            Ok(Some(response)) => response.get_error(),
            Ok(None) => "Unknown error".to_owned(),
            Err(_) => "Unknown HTTPError".to_owned(),
        };
        Err(match status {
            400 => YError { kind: Kind::BadRequest, message },
            401 | 403 | 404 | 409 | 413 => YError::other(message),
            429 => YError { kind: Kind::RateLimit, message },
            502 => YError::other("Bad Gateway"),
            _ => YError::other(format!("{message} ({status}): {}", bytes_repr(&content))),
        })
    }

    async fn retry_on_rate_limit<T, F, Fut>(&self, call: F) -> Result<T, YError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<T, YError>>,
    {
        for &delay in &self.options.rate_limit_delays {
            self.cooldown.wait().await;
            match call().await {
                Err(error) if error.kind == Kind::RateLimit => {
                    if self.cooldown.start(Duration::from_secs_f64(delay)) {
                        tracing::warn!("Rate limited, requests wait {delay:.1}s");
                    }
                }
                done => return done,
            }
        }
        self.cooldown.wait().await;
        call().await
    }

    // ---- Signing in ---------------------------------------------------------------------------------------------------

    async fn device_auth(&self) -> Result<String, YError> {
        let device_id: String = {
            use rand::RngExt as _;
            let alphanumeric = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
            let mut rng = rand::rng();
            (0..10).map(|_| alphanumeric[rng.random_range(0..alphanumeric.len())] as char).collect()
        };
        let form = vec![("client_id", CLIENT_ID.to_owned()), ("device_id", device_id), ("device_name", DEVICE_NAME.to_owned())];
        let code = self.request(Method::POST, &format!("{}/device/code", self.options.oauth), &[], Body::Form(form)).await?;
        let field = |name: &str| code.as_ref().and_then(|c| c.get(name));
        let (Some(device_code), Some(user_code), Some(url)) = (
            field("device_code").and_then(Value::as_str),
            field("user_code").and_then(Value::as_str),
            field("verification_url").and_then(Value::as_str),
        ) else {
            return Err(YError::other("failed to parse device code response"));
        };
        let (interval, total) = (field("interval").and_then(Value::as_f64).unwrap_or(5.0), field("expires_in").and_then(Value::as_f64).unwrap_or(300.0));
        {
            let mut state = self.st();
            state.auth_url = Some(url.to_owned());
            state.user_code = Some(user_code.to_owned());
        }
        tracing::info!("Auth: open {url} and enter code {user_code}");
        let deadline = Instant::now() + Duration::from_secs_f64(total);
        loop {
            let form = vec![
                ("grant_type", "device_code".to_owned()),
                ("code", device_code.to_owned()),
                ("client_id", CLIENT_ID.to_owned()),
                ("client_secret", CLIENT_SECRET.to_owned()),
            ];
            match self.request(Method::POST, &format!("{}/token", self.options.oauth), &[], Body::Form(form)).await {
                Ok(token) => {
                    if let Some(token) = token.as_ref().and_then(|t| t.get("access_token")).and_then(Value::as_str) {
                        return Ok(token.to_owned());
                    }
                    return Err(YError::other("no access token in the response"));
                }
                // The user has not entered the code yet.
                Err(error) if error.kind == Kind::BadRequest && error.message.contains("authorization_pending") => {}
                Err(error) => return Err(error),
            }
            if Instant::now() >= deadline {
                return Err(YError::other(format!("timed out after {total}s waiting for user confirmation")));
            }
            tokio::time::sleep(Duration::from_secs_f64(interval)).await;
        }
    }

    /// Ask Yandex for a code and wait for it to be entered; again after a failure.
    async fn init_token(me: Arc<Self>) {
        loop {
            match me.device_auth().await {
                Ok(token) => {
                    {
                        let mut state = me.st();
                        state.settings.insert("token".to_owned(), json!(token));
                        state.signed_in = true;
                        state.auth_url = None;
                        state.user_code = None;
                        state.init_task = None;
                    }
                    me.signed_in.notify_waiters();
                    if let Err(error) = me.save() {
                        tracing::error!("Could not keep the token: {error}");
                    }
                    tracing::info!("Token saved.");
                    return;
                }
                Err(error) => {
                    tracing::error!("Auth failed, retry in {RETRY_AUTH_SECONDS}s: {error}");
                    tokio::time::sleep(Duration::from_secs(RETRY_AUTH_SECONDS)).await;
                }
            }
        }
    }

    fn save(&self) -> std::io::Result<()> {
        let settings = self.st().settings.clone();
        save_settings(&self.settings_path, &settings)
    }

    // ---- The calls the radio makes ------------------------------------------------------------------------------------

    async fn rotor_station_tracks(&self, station: &str, queue: Option<&str>) -> Result<Option<Value>, YError> {
        // (the library gives `queue` instead of `settings2`, not with it)
        let params = match queue {
            Some(queue) if !queue.is_empty() && queue != "0" => vec![("queue", queue.to_owned())],
            _ => vec![("settings2", "True".to_owned())],
        };
        self.request(Method::GET, &format!("{}/rotor/station/{station}/tracks", self.options.api), &params, Body::None).await
    }

    /// Feedback about what is heard: `radioStarted`, `trackStarted`, `trackFinished`, `skip`.
    async fn rotor_station_feedback(&self, station: &str, kind: &str, from: Option<&str>, batch_id: &str, total_played_seconds: f64, track_id: Option<&str>) -> Result<bool, YError> {
        let mut data = Map::new();
        data.insert("type".to_owned(), json!(kind));
        data.insert("timestamp".to_owned(), json!(self.clock.wall()));
        if let Some(track_id) = track_id.filter(|t| !t.is_empty() && *t != "0") {
            data.insert("trackId".to_owned(), json!(track_id));
        }
        if let Some(from) = from.filter(|f| !f.is_empty()) {
            data.insert("from".to_owned(), json!(from));
        }
        if total_played_seconds != 0.0 {
            data.insert("totalPlayedSeconds".to_owned(), json!(total_played_seconds));
        }
        let params: Vec<(&str, String)> = if batch_id.is_empty() { Vec::new() } else { vec![("batch-id", batch_id.to_owned())] };
        let result = self.request(Method::POST, &format!("{}/rotor/station/{station}/feedback", self.options.api), &params, Body::Json(Value::Object(data))).await?;
        Ok(result.as_ref().and_then(Value::as_str) == Some("ok"))
    }

    /// `_send_feedback`: feedback that fails is told in the log and nothing else.
    async fn send_feedback(&self, track_id: &str, kind: &str, station: &str, batch_id: &str, total_played_seconds: f64) {
        tracing::debug!("Feedback send, track id: {track_id}, feedback: {kind}, station id: {station}, batch_id: {batch_id}, seconds {total_played_seconds}");
        if let Err(error) = self.rotor_station_feedback(station, kind, Some(&self.from_id()), batch_id, total_played_seconds, Some(track_id)).await {
            tracing::error!("Error send feedback: {error}");
        }
    }

    /// The link to a file, from a signed request: (codec, bitrate, links).
    async fn get_download_info(&self, track_id: &str) -> Result<Option<DownloadInfo>, YError> {
        let timestamp = self.clock.wall() as i64;
        let quality = self.setting("quality");
        let codecs = if quality == "lossless" { LOSSLESS_CODECS } else { CODECS };
        let params = [("ts", timestamp.to_string()), ("trackId", track_id.to_owned()), ("quality", quality), ("codecs", codecs.to_owned()), ("transports", "raw".to_owned())];
        let signed: String = params.iter().map(|(_, v)| v.as_str()).collect::<String>().replace(',', "");
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(SIGN_KEY.as_bytes()).expect("a key of any size");
        mac.update(signed.as_bytes());
        let sign = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        let mut params = params.to_vec();
        params.push(("sign", sign[..sign.len() - 1].to_owned()));

        let response = self
            .retry_on_rate_limit(|| async {
                let _slot = self.info_slots.acquire().await.expect("the semaphore is never closed");
                self.request(Method::GET, &format!("{}/get-file-info", self.options.api), &params, Body::None).await
            })
            .await?;
        let info = response.as_ref().and_then(|r| r.get("downloadInfo")).filter(|i| i.is_object());
        let Some(info) = info else {
            tracing::warn!("Track {track_id}: no downloadInfo in response: {response:?}");
            return Ok(None);
        };
        let text = |key: &str| info.get(key).and_then(Value::as_str).map(str::to_owned);
        let (Some(codec), Some(bitrate)) = (text("codec"), info.get("bitrate").and_then(Value::as_i64)) else {
            return Err(YError::other("downloadInfo has no codec or bitrate"));
        };
        let urls = info.get("urls").and_then(Value::as_array).map(|u| u.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        Ok(Some(DownloadInfo { codec, bitrate, urls }))
    }
}

struct DownloadInfo {
    codec: String,
    bitrate: i64,
    urls: Vec<String>,
}

/// `str()` of an id that the service gives as a number or as a string.
fn id_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    }
}

/// A track of the station, as far as the radio needs it.
struct StationTrack {
    id: String,
    track_id: String,
    title: String,
    artists: Vec<String>,
    genre: String,
    cover: String,
    duration_ms: f64,
    available: bool,
}

impl StationTrack {
    fn of(track: &Value) -> Self {
        let id = id_text(track.get("id"));
        let albums = track.get("albums").and_then(Value::as_array).cloned().unwrap_or_default();
        let track_id = match albums.first() {
            Some(album) => format!("{id}:{}", id_text(album.get("id"))),
            None => id,
        };
        let genre = albums.iter().find_map(|a| a.get("genre").and_then(Value::as_str).filter(|g| !g.is_empty()).map(|g| g.trim().to_owned())).unwrap_or_default();
        let cover = ["coverUri", "ogImage"]
            .into_iter()
            .find_map(|key| track.get(key).and_then(Value::as_str).filter(|c| !c.is_empty()))
            .map_or_else(String::new, |uri| if uri.starts_with("http") { uri.to_owned() } else { format!("https://{uri}") });
        Self {
            id: id_text(track.get("id")),
            track_id,
            title: track.get("title").and_then(Value::as_str).unwrap_or("").to_owned(),
            artists: track.get("artists").and_then(Value::as_array).into_iter().flatten().filter_map(|a| a.get("name").and_then(Value::as_str)).filter(|n| !n.is_empty()).map(str::to_owned).collect(),
            genre,
            cover,
            duration_ms: track.get("durationMs").and_then(Value::as_f64).unwrap_or(0.0),
            available: track.get("available").and_then(Value::as_bool).unwrap_or(false),
        }
    }
}

#[async_trait]
impl MusicBackend for YandexBackend {
    fn authorized(&self) -> bool {
        !self.st().settings.get("token").is_none_or(Value::is_null)
    }

    /// The auth page and the code for it, while there's no token yet.
    fn auth_prompt(&self) -> AuthPrompt {
        let state = self.st();
        AuthPrompt { url: state.auth_url.clone(), code: state.user_code.clone(), fields: Vec::new() }
    }

    fn default_station(&self) -> String {
        DEFAULT_STATION.to_owned()
    }

    async fn wait_authorized(&self) {
        loop {
            let notified = self.signed_in.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.st().signed_in {
                return;
            }
            notified.await;
        }
    }

    /// Ask Yandex for a code to enter on its page, until it is entered.
    fn start_auth(&self) {
        if self.authorized() {
            return;
        }
        let mut state = self.st();
        if state.init_task.is_some() {
            return;
        }
        if let Some(me) = self.me.upgrade() {
            state.init_task = Some(tokio::spawn(Self::init_token(me)));
        }
    }

    async fn authorize(&self, _values: &HashMap<String, String>) -> Result<(), AuthError> {
        Err(AuthError("Yandex signs in by the code on its page".to_owned()))
    }

    fn supports_moods(&self) -> bool {
        true
    }

    /// One batch of station tracks, ready to be downloaded.
    async fn station_batch(&self, station_id: &str, exclude_track_ids: &HashSet<String>) -> Result<Vec<Track>, BackendError> {
        let queue = self.st().last_track.clone();
        let Some(station_tracks) = self.rotor_station_tracks(station_id, queue.as_deref()).await? else {
            return Err(BackendError(format!("No tracks for station {station_id}")));
        };
        let batch_id = station_tracks.get("batchId").and_then(Value::as_str).unwrap_or("").to_owned();

        let changed = {
            let mut state = self.st();
            let now = (station_id.to_owned(), batch_id.clone());
            let changed = state.last_station != now;
            if changed {
                state.last_station = now;
            }
            changed
        };
        if changed {
            self.rotor_station_feedback(station_id, "radioStarted", Some(&self.from_id()), &batch_id, 0.0, None).await?;
        }

        let sequence = station_tracks.get("sequence").and_then(Value::as_array).cloned().unwrap_or_default();
        let candidates: Vec<StationTrack> = sequence
            .iter()
            .filter_map(|item| item.get("track").filter(|t| t.is_object()))
            .map(StationTrack::of)
            .filter(|track| !exclude_track_ids.contains(&track.track_id) && track.available)
            .collect();

        // The edges of the batch: the first track has begun, the last one is over.
        let tracks_of = |item: Option<&Value>| item.and_then(|i| i.get("track")).filter(|t| t.is_object()).map(StationTrack::of);
        let (first, last) = (tracks_of(sequence.first()), tracks_of(sequence.last()));
        if sequence.is_empty() {
            return Err(BackendError("the batch has no tracks".to_owned()));
        }
        if let Some(first) = first {
            self.send_feedback(&first.track_id, "trackStarted", station_id, &batch_id, 0.0).await;
        }
        if let Some(last) = last {
            self.send_feedback(&last.track_id, "trackFinished", station_id, &batch_id, last.duration_ms / 1000.0).await;
            self.st().last_track = Some(last.track_id);
        }

        let loaded = futures_util::future::join_all(candidates.iter().map(|track| self.get_download_info(&track.id))).await;
        // Tracks come with the mood the station was set to when they came.
        let mood = self.st().mood.clone().filter(|m| m != "all").unwrap_or_default();
        let mut tracks = Vec::new();
        for (track, info) in candidates.into_iter().zip(loaded) {
            let Some(info) = info? else { continue };
            tracks.push(Track {
                track_id: track.track_id,
                title: track.title,
                artists: track.artists,
                codec: info.codec,
                bitrate_in_kbps: info.bitrate,
                genre: track.genre,
                cover: track.cover,
                batch_id: batch_id.clone(),
                mood: mood.clone(),
                source: String::new(),
                native_id: String::new(),
            });
        }
        Ok(tracks)
    }

    async fn download(&self, track: &Track, path: &Path) -> Result<(), BackendError> {
        let Some(info) = self.get_download_info(&track.track_id).await? else { return Err(BackendError(format!("no download links for {}", track.track_id))) };
        tracing::debug!("Track {}, download info: codec {}, bitrate {}", track.track_id, info.codec, info.bitrate);
        if track.bitrate_in_kbps != info.bitrate {
            tracing::warn!("Track {}, bitrate not match: {} != {}", track.track_id, track.bitrate_in_kbps, info.bitrate);
            return Err(BackendError(format!("no download links for {}", track.track_id)));
        }
        if track.codec != info.codec {
            tracing::warn!("Track {}, codec not match: {} != {}", track.track_id, track.codec, info.codec);
            return Err(BackendError(format!("no download links for {}", track.track_id)));
        }
        let Some(link) = info.urls.first() else { return Err(BackendError(format!("no download links for {}", track.track_id))) };
        let fail = |error: &dyn std::fmt::Display| BackendError(format!("download of {} from {}: {error}", track.track_id, where_(link)));
        let response = self.http.get(link).send().await.map_err(|e| fail(&e))?.error_for_status().map_err(|e| fail(&e))?;
        let mut file = tokio::fs::File::create(path).await.map_err(|e| fail(&e))?;
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt as _;
        while let Some(chunk) = stream.next().await {
            file.write_all(&chunk.map_err(|e| fail(&e))?).await.map_err(|e| fail(&e))?;
        }
        file.flush().await.map_err(|e| fail(&e))
    }

    async fn feedback_track(&self, track: &Track, feedback: Feedback, station_id: &str, total_played_seconds: f64) -> Result<(), BackendError> {
        if track.batch_id.is_empty() {
            return Ok(());
        }
        let kind = match feedback {
            Feedback::Started => "trackStarted",
            Feedback::Finished => "trackFinished",
            Feedback::Skipped => "skip",
        };
        self.send_feedback(&track.track_id, kind, station_id, &track.batch_id, total_played_seconds).await;
        Ok(())
    }

    /// The station's (mood_energy, diversity, language).
    async fn station_settings(&self, station_id: &str) -> Result<(String, String, String), BackendError> {
        let info = self.request(Method::GET, &format!("{}/rotor/station/{station_id}/info", self.options.api), &[], Body::None).await?;
        let current = info.as_ref().and_then(|i| i.as_array()).and_then(|a| a.first()).and_then(|r| r.get("settings2")).filter(|s| s.is_object());
        let Some(current) = current else { return Ok(("all".to_owned(), "default".to_owned(), "any".to_owned())) };
        let text = |key: &str, default: &str| current.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()).unwrap_or(default).to_owned();
        Ok((text("moodEnergy", "all"), text("diversity", "default"), text("language", "any")))
    }

    /// Change the station's settings. They are kept on the account.
    async fn set_station_settings(&self, station_id: &str, mood_energy: &str, diversity: &str, language: &str) -> Result<(), BackendError> {
        tracing::info!("Station {station_id}: mood {mood_energy}, tracks {diversity}, language {language}");
        let mut data = Map::new();
        data.insert("moodEnergy".to_owned(), json!(mood_energy));
        data.insert("diversity".to_owned(), json!(diversity));
        data.insert("type".to_owned(), json!("rotor"));
        if !language.is_empty() {
            data.insert("language".to_owned(), json!(language));
        }
        let result = self.request(Method::POST, &format!("{}/rotor/station/{station_id}/settings3", self.options.api), &[], Body::Json(Value::Object(data))).await?;
        if result.as_ref().and_then(Value::as_str) != Some("ok") {
            tracing::warn!("The station did not accept the new settings");
            return Ok(());
        }
        self.st().mood = Some(mood_energy.to_owned());
        Ok(())
    }

    async fn aclose(&self) {
        let task = self.st().init_task.take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }
}
