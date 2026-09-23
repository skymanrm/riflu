pub mod api;
pub mod auth;
pub mod error;
pub mod media;
/// macOS only: Tauri installs a default menu there and nowhere else.
#[cfg(target_os = "macos")]
pub mod menu;
pub mod model;
pub mod rotor;
pub mod settings;
pub mod store;
pub mod tray;

use std::collections::HashSet;
use std::sync::Arc;

use serde::Serialize;
use tauri::tray::TrayIcon;
use tauri::{AppHandle, LogicalSize, LogicalUnit, Manager, WindowEvent, WindowSizeConstraints};
use tokio::sync::{Mutex, RwLock};

use api::Api;
use auth::DeviceCode;
use error::{Error, Result};
use model::{AccountView, StationView, TrackView};
use rotor::{WaveSession, MY_WAVE};
use settings::Settings;

/// The window is fixed-width and resizes vertically only, so each mode is one
/// width plus a height range.
const MINI_SIZE: (f64, f64) = (420.0, 66.0);
const FULL_WIDTH: f64 = 460.0;
const FULL_MIN_HEIGHT: f64 = 560.0;
const FULL_HEIGHT: f64 = 700.0;

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
    /// Window height before shrinking to the mini player, to restore on expand.
    full_height: std::sync::Mutex<Option<f64>>,
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
    let code = auth::request_device_code(&http, "yamusic").await?;
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
    settings: Settings,
) -> Result<Settings> {
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
    if previous.always_on_top != settings.always_on_top {
        let _ = w.set_always_on_top(settings.always_on_top);
    }
    if previous.mini_player != settings.mini_player {
        apply_mini(&w, state, settings.mini_player);
    }
}

/// Both modes are outside the other's size range, so the constraints have to
/// move before the size does. Per-axis constraints keep the width pinned while
/// the full player stays free to grow downwards.
fn apply_mini(w: &tauri::WebviewWindow, state: &AppState, mini: bool) {
    let logical = |v: f64| Some(LogicalUnit(v).into());
    let (constraints, size) = if mini {
        if let (Ok(size), Ok(scale)) = (w.inner_size(), w.scale_factor()) {
            let height = size.to_logical::<f64>(scale).height;
            if height > MINI_SIZE.1 {
                *state.full_height.lock().expect("size lock") = Some(height);
            }
        }
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
        let height = state
            .full_height
            .lock()
            .expect("size lock")
            .take()
            .unwrap_or(FULL_HEIGHT)
            .max(FULL_MIN_HEIGHT);
        (
            WindowSizeConstraints {
                min_width: logical(FULL_WIDTH),
                max_width: logical(FULL_WIDTH),
                min_height: logical(FULL_MIN_HEIGHT),
                max_height: None,
            },
            LogicalSize::new(FULL_WIDTH, height),
        )
    };
    // The mini player has no title bar at all.
    let _ = w.set_decorations(!mini);
    #[cfg(target_os = "macos")]
    if !mini {
        // Restoring decorations rebuilds the style mask on macOS's own
        // schedule, undoing the overlaid title bar and the disabled zoom
        // button. Both have to be re-applied once that has landed.
        let w = w.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(120)).await;
            let _ = w.set_title_bar_style(tauri::utils::TitleBarStyle::Overlay);
            let _ = w.set_maximizable(false);
        });
    }
    let _ = w.set_size_constraints(constraints);
    let _ = w.set_size(size);
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
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
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
                let _ = w.set_always_on_top(saved.always_on_top);
                if saved.mini_player {
                    apply_mini(&w, &state, true);
                }
            }

            app.manage(state);
            Ok(())
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
            wave_next,
            wave_skip_to,
            wave_queue,
            wave_restart,
            wave_feedback,
            like_track,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
