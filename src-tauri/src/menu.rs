//! The menu bar of macOS: the app, Edit, Controls, View, Window and Help menus, with the
//! shortcuts of the player (the same ones the page has on the other systems).
//!
//! The items do nothing here: a click, or its shortcut, goes to the page as the event
//! "menu" with the name of the action ("play", "next", "tab-2"...), and the page does it. The page
//! tells back what the items should show (see `update_menu`). Elsewhere (Linux, Windows) a window
//! would get a menu bar of its own, which the compact player does not want: nothing is made.

use std::sync::Mutex;

use tauri::{
    AppHandle, Wry,
    menu::{CheckMenuItem, MenuItem},
};

/// The items the page changes.
pub struct MenuItems {
    pub play: MenuItem<Wry>,
    pub next: MenuItem<Wry>,
    pub lossless: CheckMenuItem<Wry>,
}

#[derive(Default)]
pub struct MenuState(pub Mutex<Option<MenuItems>>);

/// The ids of the items of the menu bar start with this: the tray has ids of its own.
pub const PREFIX: &str = "m:";

#[cfg(target_os = "macos")]
pub fn install(app: &AppHandle) -> tauri::Result<()> {
    use tauri::{
        Manager,
        menu::{
            AboutMetadataBuilder, CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder,
            SubmenuBuilder,
        },
    };

    let item = |id: &str, text: &str, accelerator: Option<&str>| {
        let mut builder = MenuItemBuilder::with_id(format!("{PREFIX}{id}"), text);
        if let Some(accelerator) = accelerator {
            builder = builder.accelerator(accelerator);
        }
        builder.build(app)
    };

    let about = AboutMetadataBuilder::new()
        .name(Some("EndlessWave"))
        .version(Some(env!("CARGO_PKG_VERSION")))
        .license(Some("MIT"))
        .website(Some("https://github.com/vm86/endlesswave"))
        .comments(Some(
            "An endless DJ radio: tracks beat-matched and blended into one mix",
        ))
        .build();
    // The settings of a Mac app are in its menu, with the shortcut of the system: ⌘,
    let settings = item("settings", "Settings…", Some("CmdOrCtrl+,"))?;
    let updates = item("update", "Check for Updates…", None)?;
    let app_menu = SubmenuBuilder::new(app, "EndlessWave")
        .about(Some(about))
        .item(&updates)
        .separator()
        .item(&settings)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    // Needed for the text fields (the sign-in), and for copying.
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    // Space plays and pauses, but a menu shortcut of Space would eat the spaces of a text
    // field: the page keeps that key, and the item has none.
    let play = item("play", "Play", None)?;
    let next = item("next", "Next Track", Some("CmdOrCtrl+Right"))?;
    let stop = item("stop", "Stop", Some("CmdOrCtrl+."))?;
    let refresh = item("refresh", "Rebuild the Queue", Some("CmdOrCtrl+R"))?;
    let clear = item("clear", "Clear the Queue", Some("CmdOrCtrl+Shift+R"))?;
    let like = item("like", "Like / Unlike Track", Some("CmdOrCtrl+D"))?;
    let lossless =
        CheckMenuItemBuilder::with_id(format!("{PREFIX}lossless"), "Lossless Stream (FLAC)")
            .accelerator("CmdOrCtrl+L")
            .build(app)?;
    let volume_up = item("volume-up", "Volume Up", Some("CmdOrCtrl+Up"))?;
    let volume_down = item("volume-down", "Volume Down", Some("CmdOrCtrl+Down"))?;
    let controls = SubmenuBuilder::new(app, "Controls")
        .items(&[&play, &next, &stop])
        .separator()
        .items(&[&refresh, &clear, &like, &lossless])
        .separator()
        .items(&[&volume_up, &volume_down])
        .build()?;

    let tabs = [
        item("tab-1", "Playlist", Some("CmdOrCtrl+1"))?,
        item("tab-2", "Mix", Some("CmdOrCtrl+2"))?,
        item("tab-3", "Sources", Some("CmdOrCtrl+3"))?,
        item("tab-4", "Stats", Some("CmdOrCtrl+4"))?,
    ];
    let equalizer = item("equalizer", "Equalizer", Some("CmdOrCtrl+E"))?;
    let view = SubmenuBuilder::new(app, "View")
        .items(&[&tabs[0], &tabs[1], &tabs[2], &tabs[3]])
        .separator()
        .item(&equalizer)
        .build()?;

    // Closing the window only hides it to the tray (⌘W).
    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .separator()
        .close_window()
        .build()?;

    let shortcuts = item("help", "Keyboard Shortcuts", Some("CmdOrCtrl+/"))?;
    let help = SubmenuBuilder::new(app, "Help").item(&shortcuts).build()?;

    let menu = MenuBuilder::new(app)
        .items(&[&app_menu, &edit, &controls, &view, &window, &help])
        .build()?;
    app.set_menu(menu)?;

    *app.state::<MenuState>().0.lock().unwrap() = Some(MenuItems {
        play,
        next,
        lossless,
    });
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn install(_app: &AppHandle) -> tauri::Result<()> {
    Ok(())
}
