use crate::{
    audio::Audio,
    dj::{self, Dj},
    radio,
};
use radio_core::radio::{PlaybackInfo, TrackInfo};
use serde_json::{Value, json};
use std::time::Duration;
use tauri::{AppHandle, Manager};

fn request(base: &str, path: &str, body: Option<Value>) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let url = format!("{base}{path}");
    let mut response = match body {
        Some(body) => agent
            .post(&url)
            .header("content-type", "application/json")
            .send(body.to_string().as_bytes()),
        None => agent.get(&url).call(),
    }
    .map_err(|e| e.to_string())?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn snapshot(base: &str, audio: &Audio) -> Result<PlaybackInfo, String> {
    let path = audio.mix_position().map_or_else(
        || "/api/playback".to_owned(),
        |at| format!("/api/playback?at={at}"),
    );
    let result = request(base, &path, None)
        .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()));
    if result.is_err() && !audio.playing() {
        return request(base, "/api/playback", None)
            .and_then(|v| serde_json::from_value(v).map_err(|e| e.to_string()));
    }
    result
}

fn pcm_url(base: &str, track: &TrackInfo) -> String {
    let id: String = track
        .track_id
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    format!("{base}/api/track/{id}/pcm")
}

pub(crate) fn enter(app: &AppHandle) -> Result<dj::Status, String> {
    let dj = app.state::<Dj>();
    let _handoff = dj.handoff.lock().unwrap();
    if dj.is_running() {
        return Ok(dj.status());
    }
    let audio = app.state::<Audio>();
    let base = radio::url();
    let epoch = audio.epoch();
    for _ in 0..4 {
        let info = match snapshot(&base, &audio) {
            Ok(info) => info,
            Err(_error) if !audio.playing() && audio.mix_position().is_none() => {
                if app.get_webview_window("dj").is_none() {
                    return Err("the console was closed".into());
                }
                dj.prepare_empty(audio.volume());
                audio.enter_dj(app, &dj, PlaybackInfo::default(), false)?;
                if let Err(e) = request(&base, "/api/pause", Some(json!({}))) {
                    eprintln!("could not hold the automatic mix: {e}");
                }
                return Ok(dj.status());
            }
            Err(error) => return Err(error),
        };
        let a = dj::fetch(&dj.reserve_url(0, &pcm_url(&base, &info.track)))?;
        let b = info
            .next
            .as_ref()
            .map(|t| dj::fetch(&dj.reserve_url(1, &pcm_url(&base, t))))
            .transpose()?;
        let latest = snapshot(&base, &audio)?;
        if audio.epoch() != epoch {
            return Err("playback changed while the console was loading".into());
        }
        if latest.track.track_id != info.track.track_id
            || latest.next.as_ref().map(|t| &t.track_id) != info.next.as_ref().map(|t| &t.track_id)
        {
            continue;
        }
        if app.get_webview_window("dj").is_none() {
            return Err("the console was closed".into());
        }
        dj.prepare(&latest, a, b, audio.volume());
        let playing = audio.playing();
        match audio.enter_dj(app, &dj, latest, playing) {
            Ok(()) => {
                if let Err(e) = request(&base, "/api/pause", Some(json!({}))) {
                    eprintln!("could not hold the automatic mix: {e}");
                }
                return Ok(dj.status());
            }
            Err(e) if e.contains("retry the handoff") => continue,
            Err(e) => return Err(e),
        }
    }
    Err("the track kept changing while the console was loading; the radio is still playing".into())
}

pub(crate) fn leave(app: &AppHandle) -> Result<(), String> {
    let dj = app.state::<Dj>();
    let _handoff = dj.handoff.lock().unwrap();
    if !dj.is_running() {
        return Ok(());
    }
    let audio = app.state::<Audio>();
    let Some(point) = dj.return_point() else {
        audio.stop(app);
        return Ok(());
    };
    let base = radio::url();
    let prepared = audio.prepare(audio.stream_url(&base));
    let result = request(
        &base,
        "/api/playback",
        Some(json!({
            "track_id": point.track.track_id,
            "position": point.position,
            "next_track_id": point.next.as_ref().map(|t| &t.track_id),
            "paused": !point.playing,
            "advance": if point.playing { point.tempo } else { 0.0 },
            "lease_owner": dj.lease_owner(),
            "heard": dj.heard_tracks(),
        })),
    )
    .and_then(|v| serde_json::from_value::<PlaybackInfo>(v).map_err(|e| e.to_string()))
    .and_then(|mapping| {
        if !dj.is_running() {
            return Err("stopped".into());
        }
        audio.set_volume(dj.master());
        audio.return_to_radio(app, &dj, prepared, mapping, !point.playing)
    });
    if result.is_err() || !audio.playing() {
        let _ = request(&base, "/api/pause", Some(json!({})));
    } else {
        let _ = request(&base, "/api/resume", Some(json!({})));
    }
    result
}
