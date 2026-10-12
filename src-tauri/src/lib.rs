mod media;
mod radio;

use media::Media;
use radio::Radio;
use tauri::{
    menu::{MenuBuilder, MenuItem, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, RunEvent, State, WindowEvent,
};

struct TrayState {
    now: MenuItem<tauri::Wry>,
    play: MenuItem<tauri::Wry>,
}

#[tauri::command]
fn radio_url() -> String {
    radio::url()
}

#[tauri::command]
fn radio_start(app: AppHandle, radio: State<Radio>) -> Result<(), String> {
    radio.start(app.path().resource_dir().ok().as_deref())
}

#[tauri::command]
fn media_metadata(media: State<Media>, title: String, artist: String, cover: String, duration: f64) {
    media.metadata(&title, &artist, &cover, duration);
}

#[tauri::command]
fn media_playback(media: State<Media>, state: String, progress: f64) {
    media.playback(&state, progress);
}

/// The webview owns the audio, so it tells the tray what plays.
#[tauri::command]
fn update_tray(app: AppHandle, state: State<TrayState>, title: String, playing: bool) {
    let _ = state.now.set_text(if title.is_empty() { "Nothing playing" } else { &title });
    let _ = state.play.set_text(if playing { "Pause" } else { "Play" });
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(if title.is_empty() { "yamusic-radio" } else { &title }));
    }
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Radio::default())
        .manage(Media::default())
        .invoke_handler(tauri::generate_handler![radio_url, radio_start, update_tray, media_metadata, media_playback])
        .setup(|app| {
            let now = MenuItemBuilder::with_id("now", "Nothing playing").enabled(false).build(app)?;
            let play = MenuItemBuilder::with_id("play", "Play").build(app)?;
            let next = MenuItemBuilder::with_id("next", "Next track").build(app)?;
            let show = MenuItemBuilder::with_id("show", "Show window").build(app)?;
            let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&now, &play, &next])
                .separator()
                .items(&[&show, &quit])
                .build()?;
            app.manage(TrayState { now, play });

            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("yamusic-radio")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "play" => {
                        let _ = app.emit("tray-play-pause", ());
                    }
                    "next" => {
                        let _ = app.emit("tray-next", ());
                    }
                    "show" => show_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_window(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            // Not fatal: without the system controls the player still plays.
            if let Err(error) = app.state::<Media>().start(app.handle()) {
                eprintln!("media controls are not available: {error}");
            }
            Ok(())
        })
        // The close button hides the player to the tray; "Quit" really quits.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            app.state::<Radio>().stop();
        }
    });
}
