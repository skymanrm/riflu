//! Menu-bar presence: left click toggles playback, right click opens the menu.
//! Actions become events — the webview owns the audio element.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

use crate::error::Result;

pub const EVT_PLAY_PAUSE: &str = "tray:play-pause";
pub const EVT_NEXT: &str = "tray:next";

pub fn build(app: &AppHandle) -> Result<TrayIcon> {
    let play_pause = MenuItem::with_id(app, "play-pause", "Play / Pause", true, None::<&str>)?;
    let next = MenuItem::with_id(app, "next", "Next track", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", label(app), true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Riflu", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &play_pause,
            &next,
            &PredefinedMenuItem::separator(app)?,
            &toggle,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    // macOS gets a monochrome silhouette to tint; the tray elsewhere shows
    // colour, so it keeps the app icon.
    #[cfg(target_os = "macos")]
    let icon = tauri::include_image!("icons/tray.png");
    #[cfg(not(target_os = "macos"))]
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| crate::error::Error::Api("no default window icon for the tray".into()))?;

    let tray = TrayIconBuilder::with_id("main")
        .icon(icon)
        // Render as a silhouette so it works in both light and dark menu bars.
        .icon_as_template(true)
        .tooltip("Riflu")
        .menu(&menu)
        // Left click is play/pause, so the menu belongs on right click only.
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "play-pause" => {
                let _ = app.emit(EVT_PLAY_PAUSE, ());
            }
            "next" => {
                let _ = app.emit(EVT_NEXT, ());
            }
            "toggle" => {
                if window_shown(app) {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.hide();
                    }
                } else {
                    show_window(app);
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(move |tray, event| match event {
            // Hovering always precedes the right click, so the label is fresh
            // however the window was hidden (close button, ⌘H, this menu).
            TrayIconEvent::Enter { .. } => {
                let _ = toggle.set_text(label(tray.app_handle()));
            }
            // Both Down and Up arrive; acting on one avoids a double toggle.
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } => {
                let _ = tray.app_handle().emit(EVT_PLAY_PAUSE, ());
            }
            _ => {}
        })
        .build(app)?;

    Ok(tray)
}

/// On screen, as opposed to hidden or minimised.
fn window_shown(app: &AppHandle) -> bool {
    app.get_webview_window("main").is_some_and(|w| {
        w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false)
    })
}

fn label(app: &AppHandle) -> &'static str {
    if window_shown(app) { "Hide player" } else { "Show player" }
}

/// In menu-bar mode with the window hidden, this is the only way back.
pub fn show_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
