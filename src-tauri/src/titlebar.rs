//! The app draws its own window buttons on every platform. macOS keeps its
//! frame (shadow, corners, resizing, ⌘W) but loses the traffic lights.

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSWindow, NSWindowButton,
    NSWindowStyleMask, NSWindowTitleVisibility,
};
use tauri::WebviewWindow;

/// Main thread only. Borderless for the mini player, or the full player's
/// overlaid title bar, in one step: going through Tauri's decorations draws a
/// standard title bar (buttons, content pushed down) until it is restyled.
pub fn set_framed(w: &WebviewWindow, framed: bool) {
    let Ok(ptr) = w.ns_window() else { return };
    // SAFETY: as in `hide_buttons`.
    let Some(ns) = (unsafe { (ptr as *const NSWindow).as_ref() }) else { return };
    let resizable = ns.styleMask() & NSWindowStyleMask::Resizable;
    let mask = if framed {
        NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::FullSizeContentView
    } else {
        NSWindowStyleMask::Borderless
    };
    // A restyle drops keyboard focus; the search field should keep it.
    let responder = ns.firstResponder();
    ns.setStyleMask(mask | resizable);
    if framed {
        ns.setTitlebarAppearsTransparent(true);
        ns.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        if let Some(zoom) = ns.standardWindowButton(NSWindowButton::ZoomButton) {
            zoom.setEnabled(false);
        }
    }
    hide_buttons(w);
    if let Some(r) = responder {
        ns.makeFirstResponder(Some(&r));
    }
}

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

/// Main thread only. Tauri's drag hands AppKit's `currentEvent` over as the
/// anchor, which after an activating click is already the mouse-up and makes
/// the window jump; so drag only while the button is held, from where it is.
pub fn drag(w: &WebviewWindow) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    if NSEvent::pressedMouseButtons() & 1 == 0 {
        return;
    }
    let Ok(ptr) = w.ns_window() else { return };
    // SAFETY: as in `hide_buttons`.
    let Some(ns) = (unsafe { (ptr as *const NSWindow).as_ref() }) else { return };
    let current = NSApplication::sharedApplication(mtm).currentEvent();
    let event = match current {
        Some(e) if matches!(e.r#type(), NSEventType::LeftMouseDown | NSEventType::LeftMouseDragged)
            && e.windowNumber() == ns.windowNumber() =>
        {
            Some(e)
        }
        other => NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            NSEventType::LeftMouseDown,
            ns.convertPointFromScreen(NSEvent::mouseLocation()),
            NSEventModifierFlags::empty(),
            other.map_or(0.0, |e| e.timestamp()),
            ns.windowNumber(),
            None,
            0,
            1,
            1.0,
        ),
    };
    if let Some(event) = event {
        ns.performWindowDragWithEvent(&event);
    }
}
