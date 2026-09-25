pub mod api;
pub mod auth;
pub mod error;
#[cfg(target_os = "macos")]
pub mod island;
pub mod media;
/// macOS only: Tauri installs a default menu there and nowhere else.
#[cfg(target_os = "macos")]
pub mod menu;
pub mod model;
pub mod rotor;
pub mod settings;
pub mod store;
#[cfg(target_os = "macos")]
pub mod titlebar;
pub mod tray;

use std::collections::HashSet;
use std::sync::Arc;

use serde::Serialize;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIcon;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, LogicalUnit, Manager, PhysicalPosition,
    WindowEvent, WindowSizeConstraints,
};
use tokio::sync::{Mutex, RwLock};

use api::Api;
use auth::DeviceCode;
use error::{Error, Result};
use model::{AccountView, PodcastView, SearchAllView, StationView, TrackView};
use rotor::{WaveSession, MY_WAVE};
use settings::Settings;

/// The window is fixed-width and resizes vertically only, so each mode is one
/// width plus a height range.
const MINI_SIZE: (f64, f64) = (320.0, 66.0);
const FULL_WIDTH: f64 = 460.0;
const FULL_MIN_HEIGHT: f64 = 560.0;
const FULL_HEIGHT: f64 = 700.0;

/// Physical window positions per size mode. A mode returns to where it last
/// was unless the window has been moved since the previous switch.
#[derive(Default)]
struct Placement {
    full: Option<PhysicalPosition<i32>>,
    /// Logical height of the full player before it shrank.
    full_height: Option<f64>,
    mini: Option<PhysicalPosition<i32>>,
    /// Where the current mode was put, to tell whether it has moved since.
    placed: Option<PhysicalPosition<i32>>,
}

impl Placement {
    /// A few pixels of slack: the frame can settle slightly after a restyle.
    fn moved(&self, here: PhysicalPosition<i32>) -> bool {
        self.placed
            .map_or(true, |p| (p.x - here.x).abs() > 4 || (p.y - here.y).abs() > 4)
    }

    /// Clamp a window of `size` (logical) at `at` into the monitor's work area.
    fn fit(
        at: PhysicalPosition<i32>,
        size: LogicalSize<f64>,
        monitor: &tauri::Monitor,
    ) -> PhysicalPosition<i32> {
        let area = monitor.work_area();
        let size = size.to_physical::<i32>(monitor.scale_factor());
        let max_x = area.position.x + area.size.width as i32 - size.width;
        let max_y = area.position.y + area.size.height as i32 - size.height;
        PhysicalPosition::new(
            at.x.min(max_x).max(area.position.x),
            at.y.min(max_y).max(area.position.y),
        )
    }
}

#[derive(Default)]
struct AppState {
    api: RwLock<Option<Arc<Api>>>,
    account: RwLock<Option<AccountView>>,
    wave: Mutex<WaveSession>,
    pending: Mutex<Option<DeviceCode>>,
    /// Read from the sync event loop too, and never held across an await,
    /// so these use std locks rather than tokio's.
    settings: std::sync::RwLock<Settings>,
    /// Built once at startup and shown/hidden — recreating it flickers.
    tray: std::sync::Mutex<Option<TrayIcon>>,
    /// Where each size mode last sat, to put the window back on switching.
    placement: std::sync::Mutex<Placement>,
    /// The window as it was before becoming the island; set while it is one.
    #[cfg(target_os = "macos")]
    island: std::sync::Mutex<Option<island::Saved>>,
}

impl AppState {
    async fn api(&self) -> Result<Arc<Api>> {
        self.api.read().await.clone().ok_or(Error::NotAuthenticated)
    }

    /// The station to play, falling back to Моя волна.
    fn station(&self) -> String {
        let guard = self.settings.read().expect("settings lock");
        guard.station_id.clone().unwrap_or_else(|| MY_WAVE.to_string())
    }

    /// Bring the client up from a stored token and cache the account.
    async fn activate(&self, token: auth::OAuthToken) -> Result<AccountView> {
        let api = Arc::new(Api::new(token));
        let account = api.account_status().await?;
        let station = self.station();
        *self.api.write().await = Some(api);
        *self.account.write().await = Some(account.clone());
        *self.wave.lock().await = WaveSession::new(&station);
        Ok(account)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Playable {
    track: TrackView,
    url: String,
}

// ---- auth ----

/// Try the stored token. Returns the account when it still works.
#[tauri::command]
async fn auth_restore(state: tauri::State<'_, AppState>) -> Result<Option<AccountView>> {
    let Some(token) = store::load()? else {
        return Ok(None);
    };
    match state.activate(token).await {
        Ok(a) => Ok(Some(a)),
        // A dead token should present as "logged out", not as an error screen.
        Err(Error::NotAuthenticated) | Err(Error::DeviceAuth(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

#[tauri::command]
async fn auth_begin(state: tauri::State<'_, AppState>) -> Result<DeviceCode> {
    let http = reqwest::Client::new();
    let code = auth::request_device_code(&http, "Riflu").await?;
    *state.pending.lock().await = Some(code.clone());
    Ok(code)
}

/// Poll once. `None` means the user has not confirmed in the browser yet.
#[tauri::command]
async fn auth_poll(state: tauri::State<'_, AppState>) -> Result<Option<AccountView>> {
    let device_code = state
        .pending
        .lock()
        .await
        .as_ref()
        .map(|c| c.device_code.clone())
        .ok_or_else(|| Error::DeviceAuth("no sign-in in progress".into()))?;

    let http = reqwest::Client::new();
    let Some(token) = auth::poll_device_token(&http, &device_code).await? else {
        return Ok(None);
    };

    store::save(&token)?;
    *state.pending.lock().await = None;
    Ok(Some(state.activate(token).await?))
}

#[tauri::command]
async fn auth_logout(state: tauri::State<'_, AppState>) -> Result<()> {
    store::clear()?;
    *state.api.write().await = None;
    *state.account.write().await = None;
    state.wave.lock().await.reset();
    Ok(())
}

// ---- settings ----

#[tauri::command]
async fn settings_get(state: tauri::State<'_, AppState>) -> Result<Settings> {
    Ok(state.settings.read().expect("settings lock").clone())
}

#[tauri::command]
async fn settings_set(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    mut settings: Settings,
) -> Result<Settings> {
    // One compact shape at a time; the island exists on macOS only.
    if settings.island {
        settings.mini_player = false;
    }
    #[cfg(not(target_os = "macos"))]
    {
        settings.island = false;
    }
    let previous = state.settings.read().expect("settings lock").clone();
    settings::save(&app, &settings)?;
    *state.settings.write().expect("settings lock") = settings.clone();
    apply_window(&app, &state, &previous, &settings);
    Ok(settings)
}

/// Lets the frontend surface the window without needing window permissions.
#[tauri::command]
fn show_window(app: AppHandle) {
    tray::show_window(&app);
}

/// Replaces Tauri's `start_dragging`, which makes the window jump on macOS.
#[tauri::command]
fn window_drag(window: tauri::WebviewWindow) {
    #[cfg(target_os = "macos")]
    titlebar::drag(&window);
    #[cfg(not(target_os = "macos"))]
    let _ = window.start_dragging();
}

/// Collapse or expand the island. `None` when it is not showing.
#[tauri::command]
async fn island_resize(app: AppHandle, expanded: bool) -> Result<Option<IslandGeometry>> {
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let handle = app.clone();
        app.run_on_main_thread(move || {
            let on = handle.state::<AppState>().island.lock().expect("island lock").is_some();
            let _ = tx.send(if on { island::Island::resize(&handle, expanded) } else { None });
        })?;
        Ok(rx.await.ok().flatten())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, expanded);
        Ok(None)
    }
}

#[cfg(target_os = "macos")]
type IslandGeometry = island::Geometry;
#[cfg(not(target_os = "macos"))]
type IslandGeometry = ();

/// Right-click menu of the mini player, which has no toolbar to hold these.
#[tauri::command]
async fn mini_menu(app: AppHandle, window: tauri::Window) -> Result<()> {
    let menu = Menu::with_items(
        &app,
        &[
            &MenuItem::with_id(&app, MENU_EXPAND, "Full player", true, None::<&str>)?,
            &PredefinedMenuItem::separator(&app)?,
            &MenuItem::with_id(&app, MENU_QUIT, "Quit Riflu", true, None::<&str>)?,
        ],
    )?;
    window.popup_menu(&menu)?;
    Ok(())
}

/// The toolbar's ⋯ menu, dropped under the button at `x`, `y` (CSS pixels).
#[tauri::command]
async fn main_menu(app: AppHandle, window: tauri::Window, x: f64, y: f64) -> Result<()> {
    let menu = Menu::with_items(
        &app,
        &[
            &MenuItem::with_id(&app, MENU_SETTINGS, "Settings…", true, None::<&str>)?,
            &PredefinedMenuItem::separator(&app)?,
            &MenuItem::with_id(&app, MENU_LOGOUT, "Sign out", true, None::<&str>)?,
        ],
    )?;
    window.popup_menu_at(&menu, LogicalPosition::new(x, y))?;
    Ok(())
}

const MENU_EXPAND: &str = "mini-expand";
const MENU_QUIT: &str = "mini-quit";
const MENU_SETTINGS: &str = "main-settings";
const MENU_LOGOUT: &str = "main-logout";
/// The frontend owns the settings round-trip, so expanding is handed to it.
const EVT_EXPAND: &str = "menu:expand";

/// Everything a settings change means for the window and the tray. Only the
/// switches that actually flipped are acted on — re-applying the mini size
/// would undo a manual resize, and re-applying the mode steals focus.
fn apply_window(app: &AppHandle, state: &AppState, previous: &Settings, settings: &Settings) {
    if previous.menu_bar_mode != settings.menu_bar_mode {
        if let Some(tray) = state.tray.lock().expect("tray lock").as_ref() {
            apply_mode(app, tray, settings.menu_bar_mode);
        }
    }
    let Some(w) = app.get_webview_window("main") else { return };
    // The island sits above the menu bar; the floating level would sink it.
    if previous.always_on_top != settings.always_on_top && !settings.island {
        let _ = w.set_always_on_top(settings.always_on_top);
    }
    if previous.mini_player != settings.mini_player {
        apply_mini(&w, state, settings.mini_player);
    }
    #[cfg(target_os = "macos")]
    if previous.island != settings.island {
        let (handle, on, on_top) = (app.clone(), settings.island, settings.always_on_top);
        let _ = app.run_on_main_thread(move || apply_island(&handle, on, on_top));
    }
}

/// Main thread only: the island swaps the window's class to a panel.
#[cfg(target_os = "macos")]
fn apply_island(app: &AppHandle, on: bool, on_top: bool) {
    let state = app.state::<AppState>();
    let mut slot = state.island.lock().expect("island lock");
    if on {
        if slot.is_none() {
            *slot = island::Island::enter(app);
        }
    } else if let Some(saved) = slot.take() {
        island::Island::leave(app, saved);
        if let Some(w) = app.get_webview_window("main") {
            titlebar::hide_buttons(&w);
            let _ = w.set_always_on_top(on_top);
        }
    }
}

/// Both modes are outside the other's size range, so the constraints have to
/// move before the size does. Per-axis constraints keep the width pinned while
/// the full player stays free to grow downwards.
fn apply_mini(w: &tauri::WebviewWindow, state: &AppState, mini: bool) {
    let logical = |v: f64| Some(LogicalUnit(v).into());
    let here = w.outer_position().ok();
    let monitor = w.current_monitor().ok().flatten();
    let mut place = state.placement.lock().expect("placement lock");
    let moved = here.map_or(true, |h| place.moved(h));

    // Remember where the outgoing mode sat before anything changes.
    if mini {
        if let (Ok(size), Ok(scale)) = (w.inner_size(), w.scale_factor()) {
            let height = size.to_logical::<f64>(scale).height;
            if height > MINI_SIZE.1 {
                place.full_height = Some(height);
            }
        }
        place.full = here;
    } else {
        place.mini = here;
    }
    let saved = if mini { place.mini } else { place.full };

    let (constraints, size) = if mini {
        (
            WindowSizeConstraints {
                min_width: logical(MINI_SIZE.0),
                max_width: logical(MINI_SIZE.0),
                min_height: logical(MINI_SIZE.1),
                max_height: logical(MINI_SIZE.1),
            },
            LogicalSize::new(MINI_SIZE.0, MINI_SIZE.1),
        )
    } else {
        let mut height = place.full_height.unwrap_or(FULL_HEIGHT);
        if let Some(m) = &monitor {
            height = height.min(m.work_area().size.height as f64 / m.scale_factor());
        }
        (
            WindowSizeConstraints {
                min_width: logical(FULL_WIDTH),
                max_width: logical(FULL_WIDTH),
                min_height: logical(FULL_MIN_HEIGHT),
                max_height: None,
            },
            LogicalSize::new(FULL_WIDTH, height.max(FULL_MIN_HEIGHT)),
        )
    };

    // Unmoved, the mode goes back where it was; moved, it opens where the
    // window is now, kept on screen.
    let target = if moved { here } else { saved.or(here) };
    let target = match (target, &monitor) {
        (Some(t), Some(m)) => Some(Placement::fit(t, size, m)),
        (t, _) => t,
    };
    place.placed = target;
    drop(place);

    // The mini player has no title bar at all; elsewhere there is never one.
    // Restyled before resizing, both queued on the main thread in order.
    #[cfg(target_os = "macos")]
    {
        let handle = w.clone();
        let _ = w.run_on_main_thread(move || titlebar::set_framed(&handle, !mini));
    }
    let _ = w.set_size_constraints(constraints);
    let _ = w.set_size(size);
    if let Some(t) = target {
        let _ = w.set_position(t);
    }
}

/// Tray-only mode: the app gives up its Dock icon (macOS) or its taskbar
/// button (Windows/Linux) and lives in the tray instead.
fn apply_mode(app: &AppHandle, tray: &TrayIcon, menu_bar: bool) {
    let _ = tray.set_visible(menu_bar);

    #[cfg(target_os = "macos")]
    let _ = app.set_dock_visibility(!menu_bar);

    #[cfg(not(target_os = "macos"))]
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_skip_taskbar(menu_bar);
    }

    // Coming back to the Dock should leave the window visible and focused,
    // which the policy switch alone does not guarantee.
    if !menu_bar {
        tray::show_window(app);
    }
}

// ---- waves ----

/// The picker's source: the personalised dashboard first (it is the only one
/// that carries Моя волна), then the rest of the catalogue.
#[tauri::command]
async fn stations(state: tauri::State<'_, AppState>) -> Result<Vec<StationView>> {
    let api = state.api().await?;
    let mut out = api.stations_dashboard().await?;
    for s in &mut out {
        s.featured = true;
    }
    let mut seen: HashSet<String> = out.iter().map(|s| s.id.clone()).collect();
    match api.stations_list().await {
        Ok(rest) => out.extend(rest.into_iter().filter(|s| seen.insert(s.id.clone()))),
        // A missing catalogue still leaves the user their own waves.
        Err(e) => eprintln!("station list failed, showing the dashboard only: {e}"),
    }
    Ok(out)
}

/// Switch waves. The session is rebuilt, so `radioStarted` fires again for the
/// new station — send any feedback for the outgoing track before calling this.
#[tauri::command]
async fn wave_set_station(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    id: String,
    name: String,
) -> Result<Settings> {
    *state.wave.lock().await = WaveSession::new(&id);
    let settings = {
        let mut s = state.settings.write().expect("settings lock");
        s.station_id = Some(id);
        s.station_name = Some(name);
        s.clone()
    };
    settings::save(&app, &settings)?;
    Ok(settings)
}

// ---- playback ----

#[tauri::command]
async fn wave_next(state: tauri::State<'_, AppState>) -> Result<Option<Playable>> {
    take_next(&state, 0).await
}

/// Jump `skip_ahead` tracks down the queue, then play the next one.
#[tauri::command]
async fn wave_skip_to(
    state: tauri::State<'_, AppState>,
    skip_ahead: usize,
) -> Result<Option<Playable>> {
    take_next(&state, skip_ahead).await
}

#[tauri::command]
async fn wave_queue(state: tauri::State<'_, AppState>) -> Result<Vec<TrackView>> {
    Ok(state.wave.lock().await.upcoming())
}

async fn take_next(state: &AppState, skip_ahead: usize) -> Result<Option<Playable>> {
    let api = state.api().await?;
    let track = {
        let mut wave = state.wave.lock().await;
        wave.drop_ahead(skip_ahead);
        wave.next_track(&api).await?
    };
    let Some(track) = track else { return Ok(None) };

    // Resolve fresh every play: stream URLs are timestamp-signed and expire.
    let url = api.track_url(&track.id).await?;
    Ok(Some(Playable { track, url }))
}

#[tauri::command]
async fn search_all(state: tauri::State<'_, AppState>, text: String) -> Result<SearchAllView> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(SearchAllView { artists: vec![], albums: vec![], tracks: vec![] });
    }
    state.api().await?.search_all(text).await
}

#[tauri::command]
async fn artist_tracks(state: tauri::State<'_, AppState>, id: String) -> Result<Vec<TrackView>> {
    state.api().await?.artist_tracks(&id).await
}

#[tauri::command]
async fn search_podcasts(
    state: tauri::State<'_, AppState>,
    text: String,
) -> Result<Vec<PodcastView>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(vec![]);
    }
    state.api().await?.search_podcasts(text).await
}

/// A music album's tracks or a podcast's episodes; both are albums.
#[tauri::command]
async fn album_tracks(state: tauri::State<'_, AppState>, id: String) -> Result<Vec<TrackView>> {
    state.api().await?.album_tracks(&id).await
}

/// Play one track outside the wave. The session is left as it is, so the
/// wave carries on from where it was once this track ends.
#[tauri::command]
async fn track_play(state: tauri::State<'_, AppState>, track: TrackView) -> Result<Playable> {
    let url = state.api().await?.track_url(&track.id).await?;
    Ok(Playable { track, url })
}

#[tauri::command]
async fn wave_restart(state: tauri::State<'_, AppState>) -> Result<()> {
    state.wave.lock().await.reset();
    Ok(())
}

/// Feedback drives the wave's adaptation, so it must be sent for every track.
#[tauri::command]
async fn wave_feedback(
    state: tauri::State<'_, AppState>,
    kind: String,
    track_id: Option<String>,
    played_seconds: Option<f64>,
) -> Result<()> {
    let api = state.api().await?;
    let (station, batch_id) = {
        let wave = state.wave.lock().await;
        (wave.station.clone(), wave.batch_id().map(str::to_owned))
    };
    api.station_feedback(
        &station,
        &kind,
        track_id.as_deref(),
        batch_id.as_deref(),
        played_seconds,
    )
    .await
}

#[tauri::command]
async fn like_track(
    state: tauri::State<'_, AppState>,
    track_id: String,
    like: bool,
) -> Result<()> {
    let api = state.api().await?;
    let uid = state
        .account
        .read()
        .await
        .as_ref()
        .map(|a| a.uid.clone())
        .ok_or(Error::NotAuthenticated)?;
    api.like_track(&uid, &track_id, like).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());
    #[cfg(target_os = "macos")]
    let builder = builder.plugin(tauri_nspanel::init());
    builder
        .setup(|app| {
            let handle = app.handle().clone();
            let loaded = settings::load(&handle);
            let menu_bar = loaded.menu_bar_mode;

            let state = AppState {
                settings: std::sync::RwLock::new(loaded),
                ..Default::default()
            };

            // Replaces the default menu, which offers fullscreen and zoom.
            // Other platforms get no menu at all, which is what they expect.
            #[cfg(target_os = "macos")]
            app.set_menu(menu::build(&handle)?)?;

            let tray = tray::build(&handle)?;
            apply_mode(&handle, &tray, menu_bar);
            *state.tray.lock().expect("tray lock") = Some(tray);

            let saved = state.settings.read().expect("settings lock").clone();
            if let Some(w) = handle.get_webview_window("main") {
                // The bar in the page draws the window buttons on every platform.
                #[cfg(target_os = "macos")]
                titlebar::hide_buttons(&w);
                #[cfg(not(target_os = "macos"))]
                let _ = w.set_decorations(false);
                if !saved.island {
                    let _ = w.set_always_on_top(saved.always_on_top);
                }
                if saved.mini_player {
                    apply_mini(&w, &state, true);
                }
            }

            app.manage(state);
            #[cfg(target_os = "macos")]
            if saved.island {
                apply_island(&handle, true, saved.always_on_top);
            }
            Ok(())
        })
        .on_menu_event(|app, event| match event.id().as_ref() {
            MENU_EXPAND => {
                let _ = app.emit(EVT_EXPAND, ());
            }
            MENU_QUIT => app.exit(0),
            // The frontend owns the settings round-trip and the screens.
            MENU_SETTINGS | MENU_LOGOUT => {
                let _ = app.emit("menu:main", event.id().as_ref());
            }
            _ => {}
        })
        .on_window_event(|window, event| {
            // In menu-bar mode the close button hides the window; quitting is
            // left to Cmd+Q and the tray menu.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                if state.settings.read().expect("settings lock").menu_bar_mode {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            auth_restore,
            auth_begin,
            auth_poll,
            auth_logout,
            stations,
            wave_set_station,
            settings_get,
            settings_set,
            show_window,
            mini_menu,
            main_menu,
            island_resize,
            window_drag,
            wave_next,
            wave_skip_to,
            wave_queue,
            wave_restart,
            search_all,
            artist_tracks,
            search_podcasts,
            album_tracks,
            track_play,
            wave_feedback,
            like_track,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
