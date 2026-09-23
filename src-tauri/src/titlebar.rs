//! The app draws its own window buttons on every platform. macOS keeps its
//! frame (shadow, corners, resizing, ⌘W) but loses the traffic lights.

use objc2_app_kit::{NSWindow, NSWindowButton};
use tauri::WebviewWindow;

/// Main thread only. A rebuilt frame view brings the buttons back, so this is
/// re-run after anything that restores the title bar.
pub fn hide_buttons(w: &WebviewWindow) {
    let Ok(ptr) = w.ns_window() else { return };
    // SAFETY: Tauri hands back the live NSWindow it owns for this webview.
    let Some(ns) = (unsafe { (ptr as *const NSWindow).as_ref() }) else { return };
    for kind in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = ns.standardWindowButton(kind) {
            button.setHidden(true);
        }
    }
}
