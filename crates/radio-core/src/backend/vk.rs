//! VK Music. Signing in is either the cookies of a session of the VK site (the app takes them from a window where the user
//! signs in; the radio swaps them for an access token, and again when it expires) or an access token that is pasted in. The
//! tracks come from the "wave" of VK (recommendations) or from the user's own music ("favourites"), through the `method/`
//! API of VK.
//!
//! The way VK answers is taken from the Vk backend of plavno (https://github.com/gmelikov/plavno, Apache-2.0): the
//! methods, their parameters and the shape of the answers, and the rewrite of the HLS link of a track into a link to a plain
//! MP3 file, and the swap of the cookies for a token. The cookies and the token are kept in the database of the radio (see
//! `SettingsStore`).
//!
//! The ids of the tracks are `owner_id_id`, as VK writes them. `Track::batch_id` holds the `access_key` of the track, which
//! VK wants back to like it.

use super::{
    AuthError, AuthField, AuthPrompt, BackendError, Choice, Feedback, MusicBackend, Track,
    yandex::SettingsStore,
};
use async_trait::async_trait;
use rand::{rng, seq::SliceRandom};
use serde_json::{Map, Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{io::AsyncWriteExt, sync::Notify, time::Instant};

/// The id of this backend in the settings.
pub const ID: &str = "vk";
pub const API: &str = "https://web.api.vk.ru/method/";
pub const SITE: &str = "https://vk.ru";
pub const LOGIN: &str = "https://login.vk.ru";
const API_VERSION: &str = "5.289";
/// The id of the VK app that the tokens are for (the one of the VK site and its apps).
const CLIENT_ID: &str = "6287487";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
const TOKEN: &str = "token";
const COOKIE: &str = "cookie";
const USER_ID: &str = "user_id";

/// The station of the recommendations of VK, and the one of the music of the user.
pub const WAVE: &str = "wave";
pub const FAVORITES: &str = "favorites";
/// The setting of where the tracks come from (one of the two stations); it wins over the station that is asked for.
pub const TRACKS: &str = "tracks";
/// Tracks asked for at once, and the most that `audio.getById` takes.
const BATCH: usize = 20;
const BY_ID_LIMIT: usize = 32;
const PAGE: usize = 200;
/// The id of the track as `audio.getById` wants it: with the access key, if there is one.
fn with_key(track: &Track) -> String {
    if track.batch_id.is_empty() {
        track.track_id.clone()
    } else {
        format!("{}_{}", track.track_id, track.batch_id)
    }
}

/// A link that is safe to fetch the audio from: HTTPS (or loopback, for tests).
fn usable(link: &str) -> bool {
    link.starts_with("https://")
        || link.starts_with("http://127.0.0.1")
        || link.starts_with("http://localhost")
}

/// The tracks of the wave that were heard lately are not offered again.
const RECENT: usize = 200;
const WAVE_TRIES: usize = 4;
/// The codes of the API that say the token is no good.
const REJECTED: [i64; 3] = [5, 1116, 1117];
const REJECTED_TEXT: &str = "VK: the token is expired or no good; sign in again";

#[derive(Debug, Clone)]
pub struct VkOptions {
    pub api: String,
    /// The site (the `Origin` of the requests) and the host that swaps the cookies for a token.
    pub site: String,
    pub login: String,
}

impl Default for VkOptions {
    fn default() -> Self {
        Self {
            api: API.to_owned(),
            site: SITE.to_owned(),
            login: LOGIN.to_owned(),
        }
    }
}

#[derive(Default)]
struct State {
    token: Option<String>,
    /// When the token that came for the cookies runs out; none for a token that was pasted in.
    token_until: Option<Instant>,
    /// The `Cookie` header of the session of the site.
    cookie: Option<String>,
    user_id: Option<i64>,
    /// `wave` or `favorites`.
    tracks: String,
    /// The ids of the music of the user not yet offered in this round.
    library: VecDeque<String>,
    recent: VecDeque<String>,
    /// Counts up for every request to the wave, as VK wants.
    mix_id: u64,
}

pub struct VkBackend {
    http: reqwest::Client,
    options: VkOptions,
    store: Arc<dyn SettingsStore>,
    ffmpeg: PathBuf,
    state: Mutex<State>,
    changed: Notify,
}

/// Whether the settings hold a token or the cookies of a session.
pub fn has_token(store: &dyn SettingsStore) -> bool {
    let kept = store.load();
    [TOKEN, COOKIE].iter().any(|key| {
        kept.get(*key)
            .and_then(Value::as_str)
            .is_some_and(|t| !t.is_empty())
    })
}

/// The id of the account from the cookie `remixuserid` of a `Cookie` header.
pub fn user_of_cookie(cookie: &str) -> Option<i64> {
    cookie.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == "remixuserid").then(|| value.trim().parse().ok())?
    })
}

/// The seconds a token lives, from the `expires` of VK: a time of the day or a number of seconds, as text or number.
fn token_lifetime(expires: Option<&Value>, now_unix: u64) -> u64 {
    let value = expires.and_then(|e| match e {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    });
    let seconds = match value {
        Some(at) if at > 1_000_000_000 => at.saturating_sub(now_unix),
        Some(left) => left,
        None => 300,
    };
    seconds.min(86_400)
}

/// `owner_id_id` of an audio of the API.
fn audio_id(audio: &Value) -> Option<String> {
    Some(format!(
        "{}_{}",
        audio.get("owner_id")?.as_i64()?,
        audio.get("id")?.as_u64()?
    ))
}

/// The track of an audio of the API.
fn parse_track(audio: &Value) -> Option<Track> {
    let id = audio_id(audio)?;
    let title = audio.get("title")?.as_str()?.trim().to_owned();
    let artist = audio
        .get("artist")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    Some(Track {
        track_id: id,
        title,
        artists: if artist.is_empty() {
            Vec::new()
        } else {
            vec![artist.to_owned()]
        },
        codec: "mp3".to_owned(),
        bitrate_in_kbps: 0,
        genre: String::new(),
        cover: cover(audio),
        batch_id: audio
            .get("access_key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
        mood: String::new(),
        source: String::new(),
        native_id: String::new(),
    })
}

/// The biggest picture of the track: of its covers, or of its album.
fn cover(audio: &Value) -> String {
    if let Some(url) = audio
        .get("track_covers")
        .and_then(Value::as_array)
        .and_then(|c| c.last())
        .and_then(Value::as_str)
    {
        return url.to_owned();
    }
    let album = audio.get("album");
    let thumb = album.and_then(|a| a.get("thumb")).or(audio.get("thumb"));
    ["photo_1200", "photo_600", "photo_300", "photo_135"]
        .iter()
        .find_map(|key| thumb?.get(key)?.as_str())
        .unwrap_or("")
        .to_owned()
}

/// The link to the file of an HLS stream of VK, `/A(/audios)?/B/index.m3u8`, is `(/audios)?/B.mp3`; a link of another kind
/// is left as it is.
pub fn mp3_link(link: &str) -> String {
    let (path, query) = match link.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (link, None),
    };
    let Some(base) = path.strip_suffix("/index.m3u8") else {
        return link.to_owned();
    };
    let is_hex = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit());
    let mut parts: Vec<&str> = base.split('/').collect();
    let Some(file) = parts.pop().filter(|s| is_hex(s)) else {
        return link.to_owned();
    };
    let has_audios = parts.last() == Some(&"audios");
    let at = parts.len() - if has_audios { 2 } else { 1 };
    if parts.len() < 3 || !is_hex(parts[at]) {
        return link.to_owned();
    }
    parts.remove(at);
    let mut out = format!("{}/{file}.mp3", parts.join("/"));
    if let Some(query) = query {
        out.push('?');
        out.push_str(query);
    }
    out
}

impl VkBackend {
    pub fn new(store: Arc<dyn SettingsStore>, options: VkOptions) -> std::io::Result<Arc<Self>> {
        let kept = store.load();
        let state = State {
            token: kept
                .get(TOKEN)
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
                .map(str::to_owned),
            cookie: kept
                .get(COOKIE)
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(str::to_owned),
            user_id: kept.get(USER_ID).and_then(Value::as_i64),
            tracks: kept
                .get(TRACKS)
                .and_then(Value::as_str)
                .filter(|t| [WAVE, FAVORITES].contains(t))
                .unwrap_or(WAVE)
                .to_owned(),
            mix_id: unix_ms(),
            ..State::default()
        };
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(15))
            .user_agent(USER_AGENT)
            .build()
            .map_err(std::io::Error::other)?;
        Ok(Arc::new(Self {
            http,
            options,
            store,
            ffmpeg: PathBuf::from("ffmpeg"),
            state: Mutex::new(state),
            changed: Notify::new(),
        }))
    }

    /// The ffmpeg that takes a stream apart, if the link of a track can't be made a file.
    pub fn with_ffmpeg(mut self: Arc<Self>, ffmpeg: PathBuf) -> Arc<Self> {
        if let Some(me) = Arc::get_mut(&mut self) {
            me.ffmpeg = ffmpeg;
        }
        self
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("not poisoned")
    }

    /// The token to call the API with: the one that was pasted in, or the one that the cookies were swapped for (asked for
    /// again when it runs out).
    async fn access_token(&self) -> Result<String, BackendError> {
        {
            let state = self.st();
            if let Some(token) = &state.token
                && state.token_until.is_none_or(|until| Instant::now() < until)
            {
                return Ok(token.clone());
            }
        }
        self.exchange_cookie().await
    }

    /// Swap the cookies of the session for a token (`login.vk.ru/?act=web_token`).
    async fn exchange_cookie(&self) -> Result<String, BackendError> {
        let (cookie, user) = {
            let state = self.st();
            (state.cookie.clone(), state.user_id)
        };
        let Some(cookie) = cookie else {
            return Err(BackendError("VK: not signed in".to_owned()));
        };
        let site = self.options.site.trim_end_matches('/');
        let response = self
            .http
            .post(format!("{}/?act=web_token", self.options.login))
            .header("Cookie", cookie)
            .header("Origin", site)
            .header("Referer", format!("{site}/"))
            .form(&[("version", "1"), ("app_id", CLIENT_ID)])
            .send()
            .await
            .map_err(|e| BackendError(format!("VK web_token: {e}")))?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let parsed: Option<Value> = serde_json::from_str(&text).ok();
        let looks = parsed.as_ref().map_or_else(|| "not JSON".to_owned(), shape);
        tracing::debug!("VK web_token: HTTP {status}, answer {looks}");
        if !status.is_success() {
            return Err(BackendError(format!("VK web_token: HTTP {status}")));
        }
        let body: Value = serde_json::from_str(&text)
            .map_err(|_| BackendError("VK web_token: not a JSON answer".to_owned()))?;
        let entry = match &body {
            Value::Array(list) => list
                .iter()
                .find(|e| e.get("user_id").and_then(Value::as_i64) == user)
                .or(list.first()),
            other => other.get("data"),
        };
        // whose the session is, when it was not known: VK says
        let entry = entry.filter(|e| {
            user.is_none_or(|user| {
                e.get("user_id")
                    .and_then(Value::as_i64)
                    .is_none_or(|id| id == user)
            })
        });
        let token = entry
            .and_then(|e| e.get("access_token"))
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| BackendError("VK: the session is over; sign in again".to_owned()))?
            .to_owned();
        let left = token_lifetime(entry.and_then(|e| e.get("expires")), unix_ms() / 1000);
        let until = Instant::now() + Duration::from_secs(left - (left / 10).min(30));
        let mut state = self.st();
        state.token = Some(token.clone());
        state.token_until = Some(until);
        if state.user_id.is_none() {
            state.user_id = entry.and_then(|e| e.get("user_id")).and_then(Value::as_i64);
        }
        Ok(token)
    }

    /// One call of the API; what comes back is the `response`. A token that VK turns down is asked for once more when there
    /// are cookies to ask with.
    async fn call(&self, method: &str, fields: &[(&str, String)]) -> Result<Value, BackendError> {
        for attempt in 0..2 {
            let token = self.access_token().await?;
            match self.call_with(method, fields, Some(token)).await {
                Err(error) if error.0 == REJECTED_TEXT => {
                    let again = attempt == 0 && self.st().cookie.is_some();
                    let mut state = self.st();
                    state.token = None;
                    state.token_until = None;
                    if !again {
                        state.cookie = None;
                        return Err(error);
                    }
                }
                other => return other,
            }
        }
        Err(BackendError(REJECTED_TEXT.to_owned()))
    }

    async fn call_with(
        &self,
        method: &str,
        fields: &[(&str, String)],
        token: Option<String>,
    ) -> Result<Value, BackendError> {
        let Some(token) = token else {
            return Err(BackendError("VK: no token".to_owned()));
        };
        let mut form: Vec<(&str, String)> = fields.to_vec();
        form.push(("v", API_VERSION.to_owned()));
        form.push(("client_id", CLIENT_ID.to_owned()));
        form.push(("access_token", token));
        let url = format!("{}{method}", self.options.api);
        let site = self.options.site.trim_end_matches('/');
        tracing::debug!("VK {method}: asking");
        let response = self
            .http
            .post(&url)
            .header("Origin", site)
            .header("Referer", format!("{site}/audio"))
            .header("X-Requested-With", "XMLHttpRequest")
            .form(&form)
            .send()
            .await
            .map_err(|e| BackendError(format!("VK {method}: {e}")))?;
        let status = response.status();
        tracing::debug!("VK {method}: HTTP {status}");
        if !status.is_success() {
            return Err(BackendError(format!("VK {method}: HTTP {status}")));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|_| BackendError(format!("VK {method}: not a JSON answer")))?;
        if let Some(error) = body.get("error") {
            let code = error.get("error_code").and_then(Value::as_i64).unwrap_or(0);
            let message = error.get("error_msg").and_then(Value::as_str).unwrap_or("");
            tracing::warn!("VK {method}: error {code}: {message}");
            if REJECTED.contains(&code) {
                return Err(BackendError(REJECTED_TEXT.to_owned()));
            }
            return Err(BackendError(format!(
                "VK {method}: access denied (code {code})"
            )));
        }
        body.get("response")
            .cloned()
            .ok_or_else(|| BackendError(format!("VK {method}: no response")))
    }

    fn user_id(&self) -> Result<i64, BackendError> {
        self.st()
            .user_id
            .ok_or_else(|| BackendError("VK: the account is not known".to_owned()))
    }

    /// The audios with these ids (`owner_id_id[_access_key]`), in batches.
    async fn audios_by_id(&self, ids: &[String]) -> Result<Vec<Value>, BackendError> {
        let mut found = Vec::new();
        for chunk in ids.chunks(BY_ID_LIMIT) {
            let answer = self
                .call("audio.getById", &[("audios", chunk.join(","))])
                .await?;
            found.extend(answer.as_array().cloned().unwrap_or_default());
        }
        Ok(found)
    }

    /// The ids of all the music of the user.
    async fn library_ids(&self) -> Result<Vec<String>, BackendError> {
        let user = self.user_id()?;
        let mut ids = Vec::new();
        let mut offset = 0;
        loop {
            let answer = self
                .call(
                    "audio.get",
                    &[
                        ("owner_id", user.to_string()),
                        ("count", PAGE.to_string()),
                        ("offset", offset.to_string()),
                    ],
                )
                .await?;
            let items = answer
                .get("items")
                .and_then(Value::as_array)
                .ok_or_else(|| BackendError("VK did not return My Music".to_owned()))?;
            if items.is_empty() {
                break;
            }
            offset += items.len();
            ids.extend(items.iter().filter_map(audio_id));
            let total = answer
                .get("count")
                .and_then(Value::as_u64)
                .unwrap_or(offset as u64);
            if offset as u64 >= total || offset >= 100_000 {
                break;
            }
        }
        Ok(ids)
    }

    /// The wave: audios that VK recommends, not heard lately.
    async fn wave(&self, exclude: &HashSet<String>) -> Result<Vec<Track>, BackendError> {
        let mut tracks: Vec<Track> = Vec::new();
        for _ in 0..WAVE_TRIES {
            let id = {
                let mut state = self.st();
                state.mix_id += 1;
                state.mix_id
            };
            let answer = self
                .call(
                    "audio.getStreamMixAudios",
                    &[
                        ("mix_id", "common".to_owned()),
                        ("options", json!({ "id": id }).to_string()),
                    ],
                )
                .await?;
            for audio in answer.as_array().into_iter().flatten() {
                let Some(track) = parse_track(audio) else {
                    continue;
                };
                let heard = self.st().recent.contains(&track.track_id);
                if heard
                    || exclude.contains(&track.track_id)
                    || tracks.iter().any(|t| t.track_id == track.track_id)
                {
                    continue;
                }
                tracks.push(track);
            }
            if tracks.len() >= BATCH / 2 {
                break;
            }
        }
        let mut state = self.st();
        for track in &tracks {
            state.recent.push_back(track.track_id.clone());
        }
        while state.recent.len() > RECENT {
            state.recent.pop_front();
        }
        Ok(tracks)
    }

    /// The music of the user in a random order, a few tracks at a time.
    async fn favorites(&self, exclude: &HashSet<String>) -> Result<Vec<Track>, BackendError> {
        if self.st().library.is_empty() {
            let mut ids = self.library_ids().await?;
            ids.shuffle(&mut rng());
            self.st().library = ids.into();
        }
        let mut tracks = Vec::new();
        while tracks.len() < BATCH {
            let ids: Vec<String> = {
                let mut state = self.st();
                let take = (BATCH * 2).min(state.library.len());
                state.library.drain(..take).collect()
            };
            if ids.is_empty() {
                break;
            }
            for audio in self.audios_by_id(&ids).await? {
                let restricted = audio.get("is_restricted").and_then(Value::as_bool) == Some(true);
                let linked = audio.get("url").and_then(Value::as_str).is_some_and(usable);
                if restricted || !linked {
                    continue;
                }
                if let Some(track) = parse_track(&audio)
                    && !exclude.contains(&track.track_id)
                {
                    tracks.push(track);
                }
            }
        }
        Ok(tracks)
    }

    /// Keep the way in: a token, or the cookies of a session (one of them is empty).
    fn save_signin(&self, token: &str, cookie: &str, user: i64) -> Result<(), AuthError> {
        let mut settings = Map::new();
        settings.insert(TOKEN.to_owned(), json!(token));
        settings.insert(COOKIE.to_owned(), json!(cookie));
        settings.insert(USER_ID.to_owned(), json!(user));
        self.store
            .save(&settings)
            .map_err(|e| AuthError(format!("could not keep the token: {e}")))
    }

    /// The audio with this id, fresh: its link is short-lived.
    async fn link(&self, track: &Track) -> Result<String, BackendError> {
        let audios = self.audios_by_id(&[with_key(track)]).await?;
        audios
            .first()
            .and_then(|a| a.get("url"))
            .and_then(Value::as_str)
            .filter(|u| usable(u))
            .map(str::to_owned)
            .ok_or_else(|| {
                BackendError(format!(
                    "VK: no link for {} (not available?)",
                    track.track_id
                ))
            })
    }
}

/// What a JSON value looks like, without the values (they may be secrets): the keys and the kinds.
fn shape(value: &Value) -> String {
    match value {
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{k}: {}", shape(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Array(list) => format!(
            "[{} items{}]",
            list.len(),
            list.first()
                .map_or(String::new(), |f| format!(", first {}", shape(f)))
        ),
        Value::String(s) => format!("str({})", s.len()),
        Value::Number(_) => "number".to_owned(),
        Value::Bool(_) => "bool".to_owned(),
        Value::Null => "null".to_owned(),
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[async_trait]
impl MusicBackend for VkBackend {
    fn authorized(&self) -> bool {
        let state = self.st();
        (state.token.is_some() || state.cookie.is_some()) && state.user_id.is_some()
    }

    fn auth_prompt(&self) -> AuthPrompt {
        AuthPrompt {
            url: None,
            code: None,
            fields: vec![AuthField {
                name: TOKEN.to_owned(),
                label: "VK access token".to_owned(),
                secret: true,
                choices: Vec::new(),
            }],
        }
    }

    fn default_station(&self) -> String {
        WAVE.to_owned()
    }

    async fn wait_authorized(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.authorized() {
                return;
            }
            notified.await;
        }
    }

    /// Nothing goes on by itself: the app brings the cookies of a session, or the token is pasted in.
    fn start_auth(&self) {}

    /// Sign in with the cookies of a session (`cookie`, and `user_id` if the cookies don't say it) or with a token. The
    /// account is checked before anything is kept.
    async fn authorize(&self, values: &HashMap<String, String>) -> Result<(), AuthError> {
        let cookie = values.get(COOKIE).map_or("", |c| c.trim()).to_owned();
        let typed = values.get(TOKEN).map_or("", |t| t.trim()).to_owned();
        if cookie.is_empty() && typed.is_empty() {
            return Err(AuthError("paste the access token of VK".to_owned()));
        }
        let (token, until, cookie, hint) = if cookie.is_empty() {
            (typed, None, String::new(), None)
        } else {
            let hint = values
                .get(USER_ID)
                .and_then(|u| u.trim().parse().ok())
                .or_else(|| user_of_cookie(&cookie));
            {
                let mut state = self.st();
                state.cookie = Some(cookie.clone());
                state.user_id = hint;
                state.token = None;
            }
            let token = match self.exchange_cookie().await {
                Ok(token) => token,
                Err(error) => {
                    self.st().cookie = None;
                    return Err(AuthError(error.0));
                }
            };
            let until = self.st().token_until;
            (token, until, cookie, hint)
        };
        let user = self
            .call_with("users.get", &[], Some(token.clone()))
            .await
            .inspect_err(|e| tracing::warn!("VK sign-in: users.get failed: {e}"))
            .map_err(|e| AuthError(e.0))?;
        tracing::debug!("VK users.get: {}", shape(&user));
        let Some(user) = user
            .get(0)
            .and_then(|u| u.get("id"))
            .and_then(Value::as_i64)
        else {
            return Err(AuthError("VK did not say whose the token is".to_owned()));
        };
        if hint.is_some_and(|id| id != user) {
            tracing::warn!("VK sign-in: the session is of {user}, not of {hint:?}");
            self.st().cookie = None;
            return Err(AuthError(
                "the session is of another account than the one that signed in".to_owned(),
            ));
        }
        // a token that came for cookies is not kept: it is asked for again
        let kept_token = if cookie.is_empty() {
            token.as_str()
        } else {
            ""
        };
        self.save_signin(kept_token, &cookie, user)?;
        {
            let mut state = self.st();
            state.token = Some(token);
            state.token_until = until;
            state.cookie = (!cookie.is_empty()).then_some(cookie);
            state.user_id = Some(user);
            state.library.clear();
        }
        tracing::info!("VK: signed in as {user}");
        self.changed.notify_waiters();
        Ok(())
    }

    /// Where the tracks come from: the wave of VK or the music of the user.
    fn settings(&self) -> Option<(HashMap<String, String>, Vec<AuthField>)> {
        let field = AuthField {
            name: TRACKS.to_owned(),
            label: "Tracks".to_owned(),
            secret: false,
            choices: [(WAVE, "VK wave"), (FAVORITES, "My music")]
                .iter()
                .map(|(value, label)| Choice {
                    value: (*value).to_owned(),
                    label: (*label).to_owned(),
                })
                .collect(),
        };
        Some((
            HashMap::from([(TRACKS.to_owned(), self.st().tracks.clone())]),
            vec![field],
        ))
    }

    async fn update_settings(&self, values: &HashMap<String, String>) -> Result<(), AuthError> {
        let Some(tracks) = values.get(TRACKS) else {
            return Err(AuthError("expected {\"tracks\": \"...\"}".to_owned()));
        };
        if ![WAVE, FAVORITES].contains(&tracks.as_str()) {
            return Err(AuthError(format!(
                "unknown tracks {}",
                crate::api::py_repr(tracks)
            )));
        }
        let mut settings = Map::new();
        settings.insert(TRACKS.to_owned(), json!(tracks));
        self.store
            .save(&settings)
            .map_err(|e| AuthError(format!("could not keep the settings: {e}")))?;
        {
            let mut state = self.st();
            state.tracks = tracks.clone();
            state.library.clear();
        }
        tracing::info!("VK: tracks from {tracks}");
        Ok(())
    }

    /// The tracks choose their mood by their sound.
    fn supports_moods(&self) -> bool {
        false
    }

    async fn station_batch(
        &self,
        station_id: &str,
        exclude_track_ids: &HashSet<String>,
    ) -> Result<Vec<Track>, BackendError> {
        let chosen = self.st().tracks.clone();
        let result = if chosen == FAVORITES || station_id == FAVORITES {
            self.favorites(exclude_track_ids).await
        } else {
            self.wave(exclude_track_ids).await
        };
        match &result {
            Ok(tracks) => tracing::info!("VK {station_id}: {} tracks", tracks.len()),
            Err(error) => tracing::warn!("VK {station_id}: {error}"),
        }
        result
    }

    /// The file of the track: the plain MP3 that the link of the stream stands for, or else the stream taken apart by
    /// ffmpeg.
    async fn download(&self, track: &Track, path: &Path) -> Result<(), BackendError> {
        let link = mp3_link(&self.link(track).await?);
        let fail = |error: &dyn std::fmt::Display| {
            BackendError(format!("download of {} from VK: {error}", track.track_id))
        };
        if link.split('?').next().is_some_and(|p| p.ends_with(".m3u8")) {
            let output = tokio::process::Command::new(&self.ffmpeg)
                .args(["-nostdin", "-loglevel", "error", "-y", "-i"])
                .arg(&link)
                .args(["-vn", "-c:a", "copy", "-f", "mp3"])
                .arg(path)
                .stdin(Stdio::null())
                .output()
                .await
                .map_err(|e| fail(&e))?;
            return if output.status.success() {
                Ok(())
            } else {
                Err(fail(&String::from_utf8_lossy(&output.stderr).trim()))
            };
        }
        let response = self
            .http
            .get(&link)
            .send()
            .await
            .map_err(|e| fail(&e))?
            .error_for_status()
            .map_err(|e| fail(&e))?;
        let mut file = tokio::fs::File::create(path).await.map_err(|e| fail(&e))?;
        let mut stream = response.bytes_stream();
        use futures_util::StreamExt as _;
        while let Some(chunk) = stream.next().await {
            file.write_all(&chunk.map_err(|e| fail(&e))?)
                .await
                .map_err(|e| fail(&e))?;
        }
        file.flush().await.map_err(|e| fail(&e))
    }

    /// VK is told nothing of what was heard.
    async fn feedback_track(
        &self,
        _track: &Track,
        _feedback: Feedback,
        _station_id: &str,
        _total_played_seconds: f64,
    ) -> Result<(), BackendError> {
        Ok(())
    }

    async fn station_settings(
        &self,
        _station_id: &str,
    ) -> Result<(String, String, String), BackendError> {
        Ok(("all".to_owned(), "default".to_owned(), "any".to_owned()))
    }

    async fn set_station_settings(
        &self,
        _station_id: &str,
        _mood_energy: &str,
        _diversity: &str,
        _language: &str,
    ) -> Result<(), BackendError> {
        Ok(())
    }

    fn supports_likes(&self) -> bool {
        true
    }

    /// VK says whether an audio is liked (`like`); where it doesn't, an audio of the user's own is.
    async fn liked_among(&self, tracks: &[Track]) -> Result<HashSet<String>, BackendError> {
        let user = self.user_id()?;
        let ids: Vec<String> = tracks.iter().map(with_key).collect();
        let mut liked = HashSet::new();
        for audio in self.audios_by_id(&ids).await? {
            let state = match audio.get("like") {
                Some(Value::Bool(b)) => *b,
                Some(Value::Number(n)) => n.as_i64() == Some(1),
                _ => audio.get("owner_id").and_then(Value::as_i64) == Some(user),
            };
            if let (true, Some(id)) = (state, audio_id(&audio)) {
                liked.insert(id);
            }
        }
        Ok(liked)
    }

    async fn set_like(&self, track: &Track, liked: bool) -> Result<(), BackendError> {
        let Some((owner, id)) = track.track_id.split_once('_') else {
            return Err(BackendError(format!("VK: bad id {}", track.track_id)));
        };
        let mut fields = vec![("owner_id", owner.to_owned()), ("audio_id", id.to_owned())];
        let method = if liked {
            if !track.batch_id.is_empty() {
                fields.push(("access_key", track.batch_id.clone()));
            }
            "audio.add"
        } else {
            "audio.delete"
        };
        self.call(method, &fields).await?;
        self.st().library.clear();
        Ok(())
    }

    async fn aclose(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_link_of_a_stream_becomes_the_link_of_a_file() {
        assert_eq!(
            mp3_link("https://cs1.vkuseraudio.net/s/v1/acmp/aaa111/bbb222/index.m3u8?extra=x"),
            "https://cs1.vkuseraudio.net/s/v1/acmp/bbb222.mp3?extra=x"
        );
        assert_eq!(
            mp3_link("https://cs1.vkuseraudio.net/p1/aaa111/audios/bbb222/index.m3u8"),
            "https://cs1.vkuseraudio.net/p1/audios/bbb222.mp3"
        );
        assert_eq!(
            mp3_link("https://host/file.mp3?x=1"),
            "https://host/file.mp3?x=1"
        );
        assert_eq!(
            mp3_link("https://host/not-hex/index.m3u8"),
            "https://host/not-hex/index.m3u8"
        );
    }

    #[test]
    fn audios_become_tracks() {
        let audio = json!({
            "owner_id": -5, "id": 7, "title": " Song ", "artist": "Band",
            "access_key": "k", "album": {"thumb": {"photo_300": "https://c/300", "photo_1200": "https://c/1200"}}
        });
        let track = parse_track(&audio).unwrap();
        assert_eq!(track.track_id, "-5_7");
        assert_eq!(track.title, "Song");
        assert_eq!(track.artists, ["Band"]);
        assert_eq!(track.batch_id, "k");
        assert_eq!(track.cover, "https://c/1200");
        assert!(parse_track(&json!({"owner_id": 1, "id": 2})).is_none());
    }
}
