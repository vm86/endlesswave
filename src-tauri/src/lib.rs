mod audio;
mod media;
mod menu;
mod radio;

use audio::{Audio, Status};
use media::Media;
use menu::MenuState;
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
    radio.start(&app)
}

#[tauri::command]
fn player_start(app: AppHandle, audio: State<Audio>, url: String, volume: f32) {
    audio.start(&app, url, volume);
}

#[tauri::command]
fn player_stop(app: AppHandle, audio: State<Audio>) {
    audio.stop(&app);
}

#[tauri::command]
fn player_volume(audio: State<Audio>, volume: f32) {
    audio.set_volume(volume);
}

#[tauri::command]
fn player_status(audio: State<Audio>) -> Status {
    audio.status()
}

#[tauri::command]
fn media_metadata(media: State<Media>, title: String, artist: String, cover: String, duration: f64) {
    media.metadata(&title, &artist, &cover, duration);
}

#[tauri::command]
fn media_playback(media: State<Media>, state: String, progress: f64) {
    media.playback(&state, progress);
}

/// On with the native player, off with the `<audio>` of the webview (it shows itself to the system).
#[tauri::command]
fn media_enabled(media: State<Media>, enabled: bool) {
    media.set_enabled(enabled);
}

/// The page tells the tray what plays.
#[tauri::command]
fn update_tray(app: AppHandle, state: State<TrayState>, title: String, playing: bool) {
    let _ = state.now.set_text(if title.is_empty() { "Nothing playing" } else { &title });
    let _ = state.play.set_text(if playing { "Pause" } else { "Play" });
    if let Some(items) = app.state::<MenuState>().0.lock().unwrap().as_ref() {
        let _ = items.play.set_text(if playing { "Pause" } else { "Play" });
    }
    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(if title.is_empty() { "yamusic-radio" } else { &title }));
    }
}

/// What the items of the menu bar show: Next is off when the radio cannot skip now, and the
/// FLAC item is checked when the lossless stream plays.
#[tauri::command]
fn update_menu(state: State<MenuState>, can_skip: bool, lossless: bool) {
    if let Some(items) = state.0.lock().unwrap().as_ref() {
        let _ = items.next.set_enabled(can_skip);
        let _ = items.lossless.set_checked(lossless);
    }
}

/// macOS: the icon in the Dock and the menu of the app, or the tray alone. The window is brought
/// back after the switch, which would otherwise leave it behind the other apps.
#[tauri::command]
fn set_dock_visible(app: AppHandle, visible: bool) {
    #[cfg(target_os = "macos")]
    {
        use tauri::ActivationPolicy;
        let _ = app.set_activation_policy(if visible { ActivationPolicy::Regular } else { ActivationPolicy::Accessory });
        let _ = app.set_dock_visibility(visible);
        show_window(&app);
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, visible);
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
        .manage(MenuState::default())
        .manage(Audio::default())
        .manage(Media::default())
        .invoke_handler(tauri::generate_handler![
            radio_url,
            radio_start,
            update_tray,
            update_menu,
            player_start,
            player_stop,
            player_volume,
            player_status,
            media_metadata,
            media_playback,
            media_enabled,
            set_dock_visible
        ])
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
            // macOS: a black-and-white template the menu bar colors by itself. Elsewhere the
            // panel may be dark or light: the colored icon of the app.
            #[cfg(target_os = "macos")]
            {
                tray = tray
                    .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
                    .icon_as_template(true);
            }
            #[cfg(not(target_os = "macos"))]
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            menu::install(app.handle())?;
            // Not fatal: without the system's controls the player still plays.
            if let Err(error) = app.state::<Media>().start(app.handle()) {
                eprintln!("media controls are not available: {error}");
            }
            Ok(())
        })
        // The menu bar (macOS): the item is an action of the page.
        .on_menu_event(|app, event| {
            if let Some(action) = event.id().as_ref().strip_prefix(menu::PREFIX) {
                let _ = app.emit("menu", action);
            }
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
