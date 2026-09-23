# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

An unofficial desktop player for Yandex Music, focused on **Моя волна** (My Wave).
Tauri v2 shell: Rust core + vanilla TypeScript frontend (no framework). macOS-targeted.

`ANALYSIS.md` is the reference document for the API itself — every non-obvious
endpoint behaviour is recorded there with an evidence tag (`[V]` verified live,
`[S]` from source, `[?]` unverified). **Read it before changing anything that
talks to the API, and update it when you learn something new.**

## Commands

```bash
make                         # lists every task
make deps dev                # install, then app + Vite with hot reload
make check test probe        # typecheck + build, unit tests, live API check
make mac install             # release .app, then replace /Applications/Riflu.app
npm run dev                  # Vite alone — UI work in a browser (Tauri invokes will fail)
npx tsc --noEmit             # typecheck frontend

cd src-tauri
cargo test --lib             # unit tests (URL signing)
cargo test --lib media::tests::builds_link_and_excludes_leading_slash_from_signature
cargo run --example probe    # end-to-end API check, no UI — see below
```

### The probe is the debugging tool

`cargo run --example probe` signs in (or reuses the stored token) and verifies
account lookup → wave batch → rotor feedback → stream URL resolution → a real
byte-serving HEAD. **When something breaks, run this first**: it isolates the
API from the UI and from the webview. It exercises the endpoints in the order
the app does, which matters (see rotor ordering below).

## Platforms

macOS is the target; Windows compiles but has never been run. Things to know
before touching anything platform-shaped:

- **Windows cannot be cross-compiled from macOS.** Tauri needs MSVC, the
  Windows SDK and WebView2; `cargo check --target x86_64-pc-windows-msvc` dies
  in the build script on the missing `llvm-rc`. `make windows` only runs on a
  Windows host.
- `store.rs` and `settings.rs` must land in the same directory. `settings.rs`
  uses Tauri's `app_config_dir()`; `store.rs` cannot, because the token is also
  read from the probe and from the 401 refresh where there is no `AppHandle` —
  so it calls `dirs::config_dir()` directly, which is exactly what
  `app_config_dir()` does. Don't reintroduce a hardcoded `$HOME/Library` path.
- The macOS-only window keys (`titleBarStyle`, `hiddenTitle`, `acceptFirstMouse`,
  `macOSPrivateApi`) stay in the base `tauri.conf.json` and are inert elsewhere.
  A `tauri.windows.conf.json` **cannot** tweak one window key: platform configs
  merge by RFC 7386, which replaces arrays wholesale, so it would have to repeat
  the entire `app.windows` entry.
- CI is `.gitea/workflows/build.yml` — a `windows-latest` and a `macos-latest`
  job, both needing host-mode Gitea runners. The macOS job builds `--bundles
  app` and zips it with `ditto`: the `.dmg` bundler drives Finder over
  AppleScript and fails without automation rights, on a runner and on a
  developer Mac alike.
- The app draws its own window buttons (notch, mini player, close) at the
  right of `.topbar`, identical on both platforms, from one `<template>` in
  `index.html`. Windows drops decorations in `setup`; macOS keeps its frame
  (shadow, corners, resizing, ⌘W) and `titlebar.rs` hides the traffic lights.
  `data-os` on `<html>` is left for platform-only copy and controls.

## Why a desktop app

`api.music.yandex.net` allowlists exactly one browser origin — the official web
player. Every other origin gets no `Access-Control-Allow-Origin`, so a static
web app cannot call it at all. Tauri's Rust HTTP client isn't subject to CORS.
This constraint is load-bearing; don't "simplify" the core into the frontend.

## Testing the UI

Synthetic input (CGEvent clicks/drags, `osascript` key events) goes to whichever
window is on top at that point, not necessarily this one. It has already landed
clicks in an unrelated browser window and flipped a setting nobody touched. So:

1. Build, `npx tsc --noEmit` and `cargo test --lib` freely.
2. Launching the app and taking screenshots is read-only and needs no approval.
   Get the window id from `CGWindowListCopyWindowInfo` and use
   `screencapture -l <id>`, which captures the window even when it is not on top.
3. Anything that clicks, drags or types goes into a written test plan at the end
   of the task. **Run it only once the user has approved that plan.**
4. When driving, hit-test first: the target point must resolve to this app's
   window as the frontmost one containing it. Re-read the window bounds after
   every action that can move or resize it — most of them can.

## Architecture

The split is deliberate:

- **Rust owns all state**: auth, token, API client, and the wave session.
- **The frontend owns only the `<audio>` element.** It asks for "the next
  track" and reports what the user did with it.

Audio deliberately plays in the webview, not in Rust: media elements aren't
subject to CORS, so `<audio src=…>` gets buffering, seeking and `MediaSession`
(OS media keys) for free.

```
src/                 frontend — api.ts (invoke bindings), main.ts (player + screens)
src-tauri/src/
  auth.rs            OAuth device-code flow
  store.rs           token at rest (0600 file)
  api.rs             typed client; the ONLY place that refreshes on 401
  media.rs           download-info → signed URL (pure, unit-tested)
  rotor.rs           wave session state machine
  settings.rs        prefs JSON
  tray.rs            menu-bar icon
  titlebar.rs        hides the macOS traffic lights (the page draws its own)
  menu.rs            app menu, minus fullscreen and zoom
  lib.rs             Tauri commands, app state, window events
```

### Auth

Yandex registers no third-party OAuth apps, so the app presents the official
Android client's `client_id` **and `client_secret`** — both are required on the
`/token` exchange. The consent screen therefore says "Yandex Music", not
"Riflu". The user's password never reaches the app.

### Token storage

A mode-0600 file under `~/Library/Application Support/ru.fanyagin.riflu/`,
**not** the keychain. macOS binds keychain ACLs to the binary's code signature,
and `cargo build` re-signs ad-hoc with a fresh hash every time — so each rebuild
looked like a new app and re-prompted. Moving to the keychain later is contained
to `store.rs`. The token is a full account credential; revocation is only
possible at yandex.ru/security/apps.

### Playback

`download-info` → XML → MD5 over a fixed salt → `https://{host}/get-mp3/…`.
This path is **MP3 only** (the URL template says so). Lossless would require the
web player's `get-file-info`, whose HMAC key rotates with every frontend release
— deliberately not used. See ANALYSIS.md §5.

**Stream URLs are timestamp-signed and expire. Resolve per play; never cache.**

### The rotor (Моя волна)

Not a playlist — a stateful server-side session where each batch depends on the
feedback sent for the previous one. Three rules, all learned the hard way and
all verified against the live API:

1. **Feedback bodies must be JSON.** Form-encoded returns
   `400 {"name":"condition is not met"}` for every event type. The reference
   Python library (`MarshalX/yandex-music-api`) still sends form data here, so
   **code copied from it fails every feedback call silently** while the rest of
   the API keeps working — and the wave quietly stops adapting.
2. **`radioStarted` comes after the first batch**, carrying its `batchId`.
   Sending it first is rejected.
3. **`settings2` and `queue` are alternatives, not combined** — `settings2=True`
   on the first fetch, `queue=<lastTrackId>` on continuations. The settings
   endpoint is `settings3`.
4. **The two station listings are not interchangeable.** `/rotor/stations/list`
   is a bare array of ~700 stations and does **not** contain `user:onyourwave`;
   only `/rotor/stations/dashboard` (4, personalised) does. The wave picker
   merges both, dashboard first. Other station types run the identical loop.

Switching stations builds a fresh `WaveSession`, so feedback for the outgoing
track must be awaited **before** the swap or it lands on the new station with
no `batchId`.

Feedback failures are logged, never allowed to interrupt playback.

### Conventions worth keeping

- All API responses are wrapped in `{"result": …}`; `Envelope<T>` in `api.rs`
  unwraps it.
- `settings` and `tray` in `AppState` use **std locks, not tokio** — the window
  event handler runs on the sync event loop, where `blocking_lock` panics.
- Track ids arrive as either JSON string or number; `model.rs` has
  `flexible_id` for this.
- WebKit only allows programmatic `audio.play()` after a real click **inside the
  page**. A tray click is not one, so `tryPlay()` surfaces the window and asks
  the user to press ▶ once rather than failing silently.
- The window is frameless (`titleBarStyle: Overlay`), so dragging comes from
  `data-tauri-drag-region="deep"` plus `core:window:allow-start-dragging` —
  which is **not** in `core:window:default`. `acceptFirstMouse: true` is what
  makes the first drag on an unfocused window work at all. `main.ts` intercepts
  the single-click drag and calls `window_drag` instead of Tauri's
  `start_dragging`. Tauri anchors the drag on AppKit's `currentEvent`, which is
  stale after an activating click, and the window jumps.
  `titlebar::drag` only drags while the button is held.
- It is fixed-width with one height per mode, so `menu.rs` replaces
  `Menu::default` — the default carries View → Toggle Full Screen and
  Window → Zoom. Don't drop back to the default menu.
- The mini player drops decorations entirely. macOS rebuilds the style mask on
  its own schedule when they come back, wiping the overlaid title bar, the
  disabled zoom button and the hidden traffic lights, so `apply_mini`
  re-applies all three after a short delay. Doing it synchronously does not
  stick. Leaving the island rebuilds the frame view too, so it re-hides them.
- A borderless window gets no corner rounding from macOS, so the mini player
  rounds its own. That needs a transparent window (`macOSPrivateApi`), and the
  background has to be painted by `#app`: a background on `html` **or** `body`
  is propagated to the window canvas, where no `border-radius` can clip it.

- The island (`island.rs`, macOS only) is hand-drawn: ActivityKit is
  `@available(macOS, unavailable)` even in the macOS 27 SDK. The main window
  is class-swapped into a non-activating `NSPanel` (`tauri-nspanel`) at level
  27, above the menu bar, and swapped back on leave. It is the same webview
  on purpose, so ▶ in the island is a real in-page click for WebKit.
- The swap hides WebKit's KVO registration on the window, so any change that
  rebuilds the frame view (titled ↔ borderless) must happen while it is the
  original class; after the swap only the non-activating bit may flip, or
  AppKit throws and `contentView` goes nil. Tauri's `set_always_on_top`
  writes the floating level and would sink the island, so it is skipped
  while the island is on.

## Unverified / open

Tracked in ANALYSIS.md §11. Notably: `dislike` as a rotor feedback type is
unconfirmed, so the player uses `skip` as the negative signal rather than guess.
Don't add unverified endpoint behaviour without probing it first.
