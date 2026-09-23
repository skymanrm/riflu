//! The island: the main window turned into a non-activating panel that sits
//! over the notch, or a free-standing pill at the top centre without one.
//! macOS offers no public API for this (ActivityKit is iOS-only), so it is
//! drawn by hand. Everything here must run on the main thread.

use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSScreen, NSWindow, NSWindowCollectionBehavior, NSWindowLevel, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewWindow};
use tauri_nspanel::{ManagerExt, WebviewWindowExt};

use panel::IslandPanel;

/// Own module: the macro brings its own AppKit imports.
mod panel {
    tauri_nspanel::tauri_panel! {
        panel!(IslandPanel {
            config: {
                // Borderless windows refuse key status by default, and without
                // it a click elsewhere never reports a blur to collapse on.
                can_become_key_window: true,
                can_become_main_window: false
            }
        })
    }
}

/// Above the menu bar (`NSMainMenuWindowLevel` is 24), as notch apps do.
const LEVEL: NSWindowLevel = 27;
/// Width of each visible strip either side of the notch.
const WING: f64 = 38.0;
/// Pill used when the screen has no notch.
const PILL: (f64, f64) = (220.0, 32.0);
/// Gap between the menu bar and the pill.
const PILL_GAP: f64 = 6.0;
/// Room for the expanded row under the notch.
const EXPANDED: (f64, f64) = (400.0, 84.0);

/// What the window looked like before it became the island.
pub struct Saved {
    frame: NSRect,
    style: NSWindowStyleMask,
    behavior: NSWindowCollectionBehavior,
    min: NSSize,
    max: NSSize,
    shadow: bool,
}

/// Handed to the frontend so it can lay out around the notch.
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Geometry {
    /// False when drawn as a free-standing pill.
    pub notch: bool,
    pub notch_width: f64,
    pub notch_height: f64,
    pub width: f64,
    pub height: f64,
}

pub struct Island;

impl Island {
    fn ns_window(w: &WebviewWindow) -> Option<&NSWindow> {
        let ptr = w.ns_window().ok()? as *const NSWindow;
        // SAFETY: Tauri hands back the live NSWindow it owns for this webview.
        unsafe { ptr.as_ref() }
    }

    /// The notched screen if there is one, otherwise the menu-bar screen.
    fn screen(mtm: MainThreadMarker) -> Option<objc2::rc::Retained<NSScreen>> {
        let screens = NSScreen::screens(mtm);
        let notched = screens.iter().find(|s| s.safeAreaInsets().top > 0.0);
        notched.or_else(|| screens.firstObject())
    }

    /// Where the window goes, in Cocoa's bottom-left global coordinates.
    fn layout(mtm: MainThreadMarker, expanded: bool) -> Option<(NSRect, Geometry)> {
        let screen = Self::screen(mtm)?;
        let frame = screen.frame();
        let notch_height = screen.safeAreaInsets().top;
        let top = frame.origin.y + frame.size.height;
        let centre = frame.origin.x + frame.size.width / 2.0;

        let (notch_width, width, height, y) = if notch_height > 0.0 {
            let left = screen.auxiliaryTopLeftArea().size.width;
            let right = screen.auxiliaryTopRightArea().size.width;
            let notch = frame.size.width - left - right;
            let (w, h) = if expanded {
                (EXPANDED.0.max(notch + 2.0 * WING), notch_height + EXPANDED.1)
            } else {
                (notch + 2.0 * WING, notch_height)
            };
            (notch, w, h, top - h)
        } else {
            let visible = screen.visibleFrame();
            let (w, h) = if expanded { (EXPANDED.0, EXPANDED.1) } else { PILL };
            (0.0, w, h, visible.origin.y + visible.size.height - PILL_GAP - h)
        };

        let rect = NSRect::new(NSPoint::new(centre - width / 2.0, y), NSSize::new(width, height));
        let geometry = Geometry {
            notch: notch_height > 0.0,
            notch_width,
            notch_height,
            width,
            height,
        };
        Some((rect, geometry))
    }

    /// `animate` blocks until AppKit's resize animation has finished.
    fn place(ns: &NSWindow, rect: NSRect, animate: bool) {
        // Loose constraints: the ones left by the other modes would clamp the
        // frame, and pinned ones would clamp every step of the animation.
        ns.setContentMinSize(NSSize::new(0.0, 0.0));
        ns.setContentMaxSize(NSSize::new(f64::MAX, f64::MAX));
        ns.setFrame_display_animate(rect, true, animate);
    }

    /// Turn the window into the collapsed island. Returns what to restore.
    pub fn enter(app: &AppHandle) -> Option<Saved> {
        let mtm = MainThreadMarker::new()?;
        let w = app.get_webview_window("main")?;
        let ns = Self::ns_window(&w)?;
        let saved = Saved {
            frame: ns.frame(),
            style: ns.styleMask(),
            behavior: ns.collectionBehavior(),
            min: ns.contentMinSize(),
            max: ns.contentMaxSize(),
            shadow: ns.hasShadow(),
        };

        // Restyle while it is still the original class: the class swap hides
        // WebKit's KVO registration, and a frame-view rebuild after it throws.
        ns.setStyleMask(NSWindowStyleMask::Borderless);

        let panel = match w.to_panel::<IslandPanel>() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("island: could not convert the window: {e}");
                return None;
            }
        };
        // Non-activating: clicking it must not pull the app to the front.
        let _ = panel.set_style_mask(
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
        );
        panel.set_level(LEVEL as i64);
        panel.set_hides_on_deactivate(false);
        panel.set_collection_behavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel.set_has_shadow(false);
        ns.setMovable(false);

        if let Some((rect, _)) = Self::layout(mtm, false) {
            Self::place(ns, rect, false);
        }
        panel.order_front_regardless();
        Some(saved)
    }

    /// Put the window back exactly as `enter` found it.
    pub fn leave(app: &AppHandle, saved: Saved) {
        if let Ok(panel) = app.get_webview_panel("main") {
            // Only the panel bit flips here; the frame view is rebuilt below,
            // once the original (KVO-observed) class is back.
            let _ = panel.set_style_mask(NSWindowStyleMask::Borderless);
            let _ = panel.to_window();
        }
        let Some(w) = app.get_webview_window("main") else { return };
        let Some(ns) = Self::ns_window(&w) else { return };
        ns.setStyleMask(saved.style);
        ns.setCollectionBehavior(saved.behavior);
        ns.setHasShadow(saved.shadow);
        ns.setMovable(true);
        ns.setLevel(0);
        ns.setContentMinSize(saved.min);
        ns.setContentMaxSize(saved.max);
        ns.setFrame_display(saved.frame, true);
    }

    /// Collapse or expand in place, and report the geometry used.
    pub fn resize(app: &AppHandle, expanded: bool) -> Option<Geometry> {
        let mtm = MainThreadMarker::new()?;
        let w = app.get_webview_window("main")?;
        let ns = Self::ns_window(&w)?;
        let (rect, geometry) = Self::layout(mtm, expanded)?;
        Self::place(ns, rect, true);
        Some(geometry)
    }
}
