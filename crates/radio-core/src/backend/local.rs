//! A music backend that plays the audio files of a folder. Task 5.2 of docs/rust-port.md, ported from `local.py`.
//!
//! The folder is chosen on the page (or with `--local-dir`) and kept in the settings, so a restart plays from it again. Its
//! tracks are taken at random, a few at a time, and handed to the radio as files: linked, not copied, so a library costs no
//! disk. The tags (title, artist, genre) are read with ffmpeg from the tracks that are asked for, never from the whole
//! library.

use super::{AuthError, AuthField, AuthPrompt, BackendError, Feedback, MusicBackend, Track};
use crate::clock::Clock;
use async_trait::async_trait;
use rand::{SeedableRng, rngs::StdRng, seq::IndexedRandom};
use sha1::{Digest, Sha1};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
};
use tokio::sync::{Notify, Semaphore};

/// What counts as music (the extension is also what ffmpeg is given).
pub const AUDIO_SUFFIXES: [&str; 9] = ["mp3", "flac", "m4a", "aac", "ogg", "opus", "wav", "aiff", "wma"];
/// The station that takes tracks from the whole folder; any other is a subfolder of it.
pub const ALL_STATION: &str = "shuffle";
pub const BATCH_SIZE: usize = 5;
/// The folder is looked through again when its list is this old.
pub const RESCAN_SECONDS: f64 = 600.0;
const PROBES_AT_ONCE: usize = 4;
pub const UNKNOWN_ARTIST: &str = "Unknown artist";
pub const DIRECTORY: &str = "directory";

/// Find the audio files under `root`, by an id made of where they are.
pub fn scan(root: &Path) -> std::io::Result<HashMap<String, PathBuf>> {
    fn walk(root: &Path, here: &Path, found: &mut HashMap<String, PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(here)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                // What is hidden is not music; and a link to a folder is not followed (as `os.walk` does not).
                if !name.starts_with('.') && !entry.file_type()?.is_symlink() {
                    walk(root, &path, found)?;
                }
                continue;
            }
            let suffix = path.extension().map(|e| e.to_string_lossy().to_lowercase());
            // What is hidden is not music (macOS leaves "._track.mp3" files).
            if name.starts_with('.') || !suffix.is_some_and(|s| AUDIO_SUFFIXES.contains(&s.as_str())) {
                continue;
            }
            if let Ok(relative) = path.strip_prefix(root) {
                found.insert(track_id(relative), path.clone());
            }
        }
        Ok(())
    }
    let mut found = HashMap::new();
    walk(root, root, &mut found)?;
    Ok(found)
}

/// Make an id that stays the same for a file, and is safe as a name.
pub fn track_id(relative: &Path) -> String {
    let posix = relative.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
    Sha1::digest(posix.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Python's `str.splitlines()`.
fn splitlines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') {
            lines.push(&text[start..at]);
            start = at + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|&(_, next)| next == '\n') {
                chars.next();
                start += 1;
            }
        }
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// Read what `ffmpeg -f ffmetadata` writes: key=value, escaped.
pub fn parse_tags(text: &str) -> HashMap<String, String> {
    let mut tags = HashMap::new();
    for line in splitlines(text).into_iter().skip(1) {
        let Some((key, value)) = line.split_once('=') else { continue };
        if line.starts_with(';') {
            continue;
        }
        // `\(.)` -> `\1`: a backslash only says that the next character is itself.
        let mut unescaped = String::with_capacity(value.len());
        let mut chars = value.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some(next) => unescaped.push(next),
                    None => unescaped.push(c),
                }
            } else {
                unescaped.push(c);
            }
        }
        tags.insert(key.trim().to_lowercase(), unescaped.trim().to_owned());
    }
    tags
}

/// The folder that was typed, `None` if it is not one.
fn folder(typed: &str) -> Option<PathBuf> {
    let expanded = if let Some(rest) = typed.strip_prefix('~') {
        let home = std::env::var_os("HOME")?;
        if rest.is_empty() || rest.starts_with('/') { PathBuf::from(home).join(rest.trim_start_matches('/')) } else { PathBuf::from(typed) }
    } else {
        PathBuf::from(typed)
    };
    expanded.is_dir().then(|| expanded.canonicalize().unwrap_or(expanded))
}

fn link_or_copy(source: &Path, path: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    #[cfg(unix)]
    {
        if std::os::unix::fs::symlink(source, path).is_ok() {
            return Ok(());
        }
    }
    std::fs::copy(source, path).map(|_| ())
}

struct Library {
    files: Option<Arc<HashMap<String, PathBuf>>>,
    scanned_at: f64,
}

/// Plays the audio files under a directory.
pub struct LocalBackend {
    directory: Mutex<Option<PathBuf>>,
    on_directory: Option<Box<dyn Fn(&str) + Send + Sync>>,
    batch_size: usize,
    rng: Mutex<StdRng>,
    clock: Arc<dyn Clock>,
    ffmpeg: PathBuf,
    library: Mutex<Library>,
    changed: Notify,
    probes: Semaphore,
}

impl LocalBackend {
    pub fn new(directory: Option<PathBuf>, on_directory: Option<Box<dyn Fn(&str) + Send + Sync>>, clock: Arc<dyn Clock>) -> Self {
        Self {
            directory: Mutex::new(directory),
            on_directory,
            batch_size: BATCH_SIZE,
            rng: Mutex::new(StdRng::from_rng(&mut rand::rng())),
            clock,
            ffmpeg: PathBuf::from("ffmpeg"),
            library: Mutex::new(Library { files: None, scanned_at: 0.0 }),
            changed: Notify::new(),
            probes: Semaphore::new(PROBES_AT_ONCE),
        }
    }

    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// A generator of choices with a seed, for the tests.
    pub fn with_seed(self, seed: u64) -> Self {
        *self.rng.lock().expect("not poisoned") = StdRng::seed_from_u64(seed);
        self
    }

    pub fn with_ffmpeg(mut self, ffmpeg: PathBuf) -> Self {
        self.ffmpeg = ffmpeg;
        self
    }

    fn directory(&self) -> Option<PathBuf> {
        self.directory.lock().expect("not poisoned").clone()
    }

    fn setting_fields() -> Vec<AuthField> {
        vec![AuthField { name: DIRECTORY.to_owned(), label: "Folder with the music".to_owned(), secret: false }]
    }

    /// List the files of the folder, again when the list is old.
    async fn library(&self) -> Result<Arc<HashMap<String, PathBuf>>, BackendError> {
        let Some(directory) = self.directory() else { return Err(BackendError("no folder with music is chosen".to_owned())) };
        let known = {
            let library = self.library.lock().expect("not poisoned");
            let stale = self.clock.monotonic() - library.scanned_at >= RESCAN_SECONDS;
            library.files.clone().filter(|_| !stale)
        };
        if let Some(files) = known {
            return Ok(files);
        }
        let root = directory.clone();
        let files = match tokio::task::spawn_blocking(move || scan(&root)).await {
            Ok(Ok(files)) => Arc::new(files),
            Ok(Err(error)) => return Err(BackendError(format!("cannot read {}: {error}", directory.display()))),
            Err(error) => return Err(BackendError(format!("cannot read {}: {error}", directory.display()))),
        };
        {
            let mut library = self.library.lock().expect("not poisoned");
            library.files = Some(files.clone());
            library.scanned_at = self.clock.monotonic();
        }
        tracing::info!("{} audio files in {}", files.len(), directory.display());
        Ok(files)
    }

    async fn tags(&self, path: &Path) -> std::io::Result<HashMap<String, String>> {
        let _turn = self.probes.acquire().await.expect("the semaphore is never closed");
        let native = {
            let path = path.to_owned();
            tokio::task::spawn_blocking(move || tags_native(&path)).await.map_err(std::io::Error::other)?
        };
        // A file without tags that symphonia knows (a WAV with a RIFF INFO list, say) is asked of ffmpeg, if there is one.
        let native = match native {
            Some(tags) if !tags.is_empty() => return Ok(tags),
            other => other,
        };
        let output = tokio::process::Command::new(&self.ffmpeg)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(path)
            .args(["-vn", "-an", "-f", "ffmetadata", "-"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await;
        match output {
            Ok(output) => Ok(parse_tags(&String::from_utf8_lossy(&output.stdout))),
            // No ffmpeg: what symphonia found (nothing) is all there is.
            Err(_) if native.is_some() => Ok(HashMap::new()),
            Err(error) => Err(error),
        }
    }

    async fn track(&self, root: &Path, file_id: &str, path: &Path) -> std::io::Result<Track> {
        let tags = self.tags(path).await?;
        let artist = tags.get("album_artist").filter(|a| !a.is_empty()).or_else(|| tags.get("artist").filter(|a| !a.is_empty()));
        let folder = match path.parent() {
            Some(parent) if parent != root => parent.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            _ => String::new(),
        };
        let artist = artist.cloned().filter(|a| !a.is_empty()).or_else(|| Some(folder).filter(|f| !f.is_empty())).unwrap_or_else(|| UNKNOWN_ARTIST.to_owned());
        let title = tags.get("title").filter(|t| !t.is_empty()).cloned().unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
        Ok(Track {
            track_id: file_id.to_owned(),
            title,
            artists: vec![artist],
            codec: path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default(),
            bitrate_in_kbps: 0,
            genre: tags.get("genre").cloned().unwrap_or_default().to_lowercase(),
            cover: String::new(),
            batch_id: String::new(),
            mood: String::new(),
            source: String::new(),
            native_id: String::new(),
        })
    }
}

/// The tags the radio uses (`artist`, `album_artist`, `title`, `genre`, as ffmpeg names them), read by `symphonia`; `None` if
/// it can't read the file (ffmpeg is asked then).
pub fn tags_native(path: &Path) -> Option<HashMap<String, String>> {
    use symphonia::core::{
        formats::{FormatOptions, probe::Hint},
        io::MediaSourceStream,
        meta::{MetadataOptions, StandardTag},
    };
    let file = std::fs::File::open(path).ok()?;
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(extension);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, MediaSourceStream::new(Box::new(file), Default::default()), FormatOptions::default(), MetadataOptions::default())
        .ok()?;
    let mut metadata = format.metadata();
    while !metadata.is_latest() {
        metadata.pop();
    }
    let mut tags = HashMap::new();
    for tag in metadata.current().into_iter().flat_map(|revision| revision.media.tags.iter()) {
        let (key, value) = match &tag.std {
            Some(StandardTag::Artist(v)) => ("artist", v),
            Some(StandardTag::AlbumArtist(v)) => ("album_artist", v),
            Some(StandardTag::TrackTitle(v)) => ("title", v),
            Some(StandardTag::Genre(v)) => ("genre", v),
            _ => continue,
        };
        // (the last one of a key wins, and the value is trimmed, as in `parse_tags`)
        tags.insert(key.to_owned(), value.trim().to_owned());
    }
    Some(tags)
}

#[async_trait]
impl MusicBackend for LocalBackend {
    fn settings(&self) -> Option<(HashMap<String, String>, Vec<AuthField>)> {
        let directory = self.directory().map_or_else(String::new, |d| d.display().to_string());
        Some((HashMap::from([(DIRECTORY.to_owned(), directory)]), Self::setting_fields()))
    }

    /// Signing in is choosing the folder.
    fn authorized(&self) -> bool {
        self.directory().is_some_and(|d| d.is_dir())
    }

    fn auth_prompt(&self) -> AuthPrompt {
        AuthPrompt { url: None, code: None, fields: Self::setting_fields() }
    }

    fn default_station(&self) -> String {
        ALL_STATION.to_owned()
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

    /// Nothing goes on by itself: the folder is typed in.
    fn start_auth(&self) {}

    /// Choose the folder (again, too); keep it in the settings.
    async fn authorize(&self, values: &HashMap<String, String>) -> Result<(), AuthError> {
        let typed = values.get(DIRECTORY).map_or("", String::as_str).trim().to_owned();
        if typed.is_empty() {
            return Err(AuthError("type the folder with the music".to_owned()));
        }
        let probe = typed.clone();
        let directory = tokio::task::spawn_blocking(move || folder(&probe)).await.ok().flatten();
        let Some(directory) = directory else { return Err(AuthError(format!("{typed} is not a folder"))) };
        *self.directory.lock().expect("not poisoned") = Some(directory.clone());
        self.library.lock().expect("not poisoned").files = None;
        if let Some(on_directory) = &self.on_directory {
            on_directory(&directory.display().to_string());
        }
        self.changed.notify_waiters();
        tracing::info!("Local music folder: {}", directory.display());
        Ok(())
    }

    fn supports_moods(&self) -> bool {
        false
    }

    /// A few tracks at random, unheard ones if there are any.
    async fn station_batch(&self, station_id: &str, exclude_track_ids: &HashSet<String>) -> Result<Vec<Track>, BackendError> {
        let files = self.library().await?;
        let root = self.directory();
        let Some(root) = root.filter(|_| !files.is_empty()) else {
            return Err(BackendError(format!("no audio files in {}", self.directory().map_or_else(|| "None".to_owned(), |d| d.display().to_string()))));
        };
        let mut ids: Vec<&String> = files.keys().collect();
        // (sorted, so that a seed gives the same choice)
        ids.sort();
        if !station_id.is_empty() && station_id != ALL_STATION {
            let sub = root.join(station_id);
            ids.retain(|i| files[*i].starts_with(&sub));
            if ids.is_empty() {
                return Err(BackendError(format!("no audio files in {}", sub.display())));
            }
        }
        let fresh: Vec<&String> = ids.iter().copied().filter(|i| !exclude_track_ids.contains(*i)).collect();
        let pool = if fresh.is_empty() { &ids } else { &fresh };
        let chosen: Vec<String> = {
            let mut rng = self.rng.lock().expect("not poisoned");
            pool.sample(&mut *rng, self.batch_size.min(pool.len())).map(|i| (*i).clone()).collect()
        };
        let found = futures_util::future::join_all(chosen.iter().map(|id| self.track(&root, id, &files[id]))).await;
        let tracks: Vec<Track> = found.into_iter().filter_map(Result::ok).collect();
        if tracks.is_empty() {
            return Err(BackendError("none of the tracks could be read".to_owned()));
        }
        Ok(tracks)
    }

    /// Link the file to `path` (copy it where it can't be linked).
    async fn download(&self, track: &Track, path: &Path) -> Result<(), BackendError> {
        let files = self.library().await?;
        let source = files.get(&track.track_id).filter(|s| s.is_file()).cloned();
        let Some(source) = source else { return Err(BackendError(format!("{} is not in the folder any more", track.title))) };
        let (from, to) = (source.clone(), path.to_owned());
        match tokio::task::spawn_blocking(move || link_or_copy(&from, &to)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(BackendError(format!("cannot read {}: {error}", source.display()))),
            Err(error) => Err(BackendError(format!("cannot read {}: {error}", source.display()))),
        }
    }

    /// Tell nobody: a folder has none to be told.
    async fn feedback_track(&self, _track: &Track, _feedback: Feedback, _station_id: &str, _total_played_seconds: f64) -> Result<(), BackendError> {
        Ok(())
    }

    async fn station_settings(&self, _station_id: &str) -> Result<(String, String, String), BackendError> {
        Ok(("all".to_owned(), "default".to_owned(), "any".to_owned()))
    }

    /// Change nothing: a folder has no settings.
    async fn set_station_settings(&self, _station_id: &str, _mood_energy: &str, _diversity: &str, _language: &str) -> Result<(), BackendError> {
        Ok(())
    }

    async fn aclose(&self) {}
}
