//! The system's own player controls: Now Playing and the media keys on macOS
//! (MPNowPlayingInfoCenter, MPRemoteCommandCenter), MPRIS on Linux.
//!
//! The native player plays the sound (see audio.rs): this tells the system what plays and turns what
//! the system asks (a media key, Control Center, `playerctl`) into events for the page.
//!
//! It is on only while the native player is the one that plays. With the `<audio>` of the webview
//! as the player, the webview publishes itself to the system, and a second player would show up:
//! the page turns this layer off then (`set_enabled`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};
use tauri::{AppHandle, Emitter};

#[derive(Default)]
pub struct Media {
    controls: Mutex<Option<MediaControls>>,
    app: Mutex<Option<AppHandle>>,
    off: AtomicBool,
}

/// Hands the events of the system to the page.
fn attach(controls: &mut MediaControls, app: AppHandle) -> Result<(), String> {
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
        .map_err(|error| format!("{error:?}"))
}

impl Media {
    /// Needs the main thread and a running event loop: call it from `setup`.
    pub fn start(&self, app: &AppHandle) -> Result<(), String> {
        let mut controls = MediaControls::new(PlatformConfig {
            dbus_name: "endlesswave",
            display_name: "EndlessWave",
            hwnd: None,
        })
        .map_err(|error| format!("{error:?}"))?;

        attach(&mut controls, app.clone())?;
        *self.app.lock().unwrap() = Some(app.clone());
        *self.controls.lock().unwrap() = Some(controls);
        Ok(())
    }

    /// Off: this layer takes itself away from the system, so only the player that plays is there.
    /// "Stopped" is not enough for that: macOS keeps the Now Playing entry (with the track) until it
    /// is emptied, and the media keys keep going to the handlers; on Linux the player stays on D-Bus
    /// (`playerctl` lists it) until the service ends. On: it comes back.
    pub fn set_enabled(&self, enabled: bool) {
        if self.off.swap(!enabled, Ordering::SeqCst) == !enabled {
            return; // nothing changes
        }
        let mut guard = self.controls.lock().unwrap();
        let Some(controls) = guard.as_mut() else { return };
        if enabled {
            if let Some(app) = self.app.lock().unwrap().clone() {
                let _ = attach(controls, app);
            }
        } else {
            let _ = controls.set_metadata(MediaMetadata::default());
            let _ = controls.set_playback(MediaPlayback::Stopped);
            let _ = controls.detach();
        }
    }

    pub fn metadata(&self, title: &str, artist: &str, cover: &str, duration: f64) {
        if self.off.load(Ordering::SeqCst) {
            return;
        }
        if let Some(controls) = self.controls.lock().unwrap().as_mut() {
            let _ = controls.set_metadata(MediaMetadata {
                title: Some(title),
                artist: Some(artist),
                album: Some("EndlessWave"),
                cover_url: (!cover.is_empty()).then_some(cover),
                duration: (duration > 0.0).then(|| Duration::from_secs_f64(duration)),
            });
        }
    }

    /// `state` is "playing", "paused" or anything else for stopped.
    pub fn playback(&self, state: &str, progress: f64) {
        if self.off.load(Ordering::SeqCst) {
            return;
        }
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
