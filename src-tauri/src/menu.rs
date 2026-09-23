//! The macOS app menu, built explicitly instead of taken from `Menu::default`.
//! The default one carries View → Toggle Full Screen and Window → Zoom, and
//! this window has exactly one size per mode.
//!
//! macOS only: Tauri installs a default menu on macOS and nowhere else, and
//! several items here (`services`, `hide_others`, `show_all`) are documented
//! as unsupported on Windows and Linux. Those platforms get no menu.

use tauri::menu::{AboutMetadata, Menu, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Wry};

use crate::error::Result;

pub fn build(app: &AppHandle) -> Result<Menu<Wry>> {
    let about = AboutMetadata {
        name: Some("yamusic".into()),
        version: Some(env!("CARGO_PKG_VERSION").into()),
        ..Default::default()
    };

    let app_menu = Submenu::with_items(
        app,
        "yamusic",
        true,
        &[
            &PredefinedMenuItem::about(app, None, Some(about))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    // Kept for the wave search field — without it ⌘C/⌘V/⌘A do nothing there.
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    Ok(Menu::with_items(app, &[&app_menu, &edit, &window])?)
}
