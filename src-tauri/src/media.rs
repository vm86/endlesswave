//! The system's own player controls: Now Playing and the media keys on macOS
//! (MPNowPlayingInfoCenter, MPRemoteCommandCenter), MPRIS on Linux.
//!
//! The webview plays the sound; this tells the system what plays and turns
//! what the system asks (a media key, Control Center, `playerctl`) into
//! events for the page.

use std::sync::Mutex;
use std::time::Duration;

use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
use tauri::{AppHandle, Emitter};

#[derive(Default)]
pub struct Media {
    controls: Mutex<Option<MediaControls>>,
}

impl Media {
    /// Needs the main thread and a running event loop: call it from `setup`.
    pub fn start(&self, app: &AppHandle) -> Result<(), String> {
        let mut controls = MediaControls::new(PlatformConfig {
            dbus_name: "yamusic_radio_player",
            display_name: "Yamusic Radio",
            hwnd: None,
        })
        .map_err(|error| format!("{error:?}"))?;

        let app = app.clone();
        controls
            .attach(move |event| {
                let name = match event {
                    MediaControlEvent::Toggle => "media-toggle",
                    MediaControlEvent::Play => "media-play",
                    MediaControlEvent::Pause => "media-pause",
                    MediaControlEvent::Stop => "media-stop",
                    MediaControlEvent::Next => "media-next",
                    // A live radio: there is no previous track and nothing to seek in.
                    _ => return,
                };
                let _ = app.emit(name, ());
            })
            .map_err(|error| format!("{error:?}"))?;
        *self.controls.lock().unwrap() = Some(controls);
        Ok(())
    }

    pub fn metadata(&self, title: &str, artist: &str, cover: &str, duration: f64) {
        if let Some(controls) = self.controls.lock().unwrap().as_mut() {
            let _ = controls.set_metadata(MediaMetadata {
                title: Some(title),
                artist: Some(artist),
                album: Some("Yamusic Radio"),
                cover_url: (!cover.is_empty()).then_some(cover),
                duration: (duration > 0.0).then(|| Duration::from_secs_f64(duration)),
            });
        }
    }

    /// `state` is "playing", "paused" or anything else for stopped.
    pub fn playback(&self, state: &str, progress: f64) {
        if let Some(controls) = self.controls.lock().unwrap().as_mut() {
            let progress = Some(MediaPosition(Duration::from_secs_f64(progress.max(0.0))));
            let _ = controls.set_playback(match state {
                "playing" => MediaPlayback::Playing { progress },
                "paused" => MediaPlayback::Paused { progress },
                _ => MediaPlayback::Stopped,
            });
        }
    }
}
