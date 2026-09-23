import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  api,
  type Account,
  type IslandGeometry,
  type Podcast,
  type Playable,
  type Settings,
  type Station,
  type Track,
} from "./api";
import "./styles.css";

// A few controls and some copy differ per platform (the notch is macOS only).
// Set before anything renders so the stylesheet never shows the wrong ones.
document.documentElement.dataset.os = navigator.userAgent.includes("Windows")
  ? "windows"
  : "macos";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const el = {
  login: $("login"),
  loginStart: $<HTMLButtonElement>("login-start"),
  loginCode: $("login-code"),
  loginUrl: $<HTMLAnchorElement>("login-url"),
  loginUserCode: $("login-user-code"),
  loginError: $("login-error"),

  player: $("player"),
  who: $("who"),
  cover: $<HTMLImageElement>("cover"),
  title: $("title"),
  artist: $("artist"),
  elapsed: $("elapsed"),
  total: $("total"),
  seek: $("seek"),
  bar: $("bar"),
  play: $<HTMLButtonElement>("play"),
  playGlyph: $("play-glyph"),
  next: $<HTMLButtonElement>("next"),
  like: $<HTMLButtonElement>("like"),
  playerError: $("player-error"),
  queueList: $<HTMLUListElement>("queue-list"),
  openSearch: $<HTMLButtonElement>("open-search"),
  openMenu: $<HTMLButtonElement>("open-menu"),
  islandFull: $<HTMLButtonElement>("island-full"),

  stations: $("stations"),
  openStations: $<HTMLButtonElement>("open-stations"),
  closeStations: $<HTMLButtonElement>("close-stations"),
  stationName: $("station-name"),
  stationSearch: $<HTMLInputElement>("station-search"),
  stationList: $<HTMLUListElement>("station-list"),
  stationsError: $("stations-error"),

  search: $("search"),
  closeSearch: $<HTMLButtonElement>("close-search"),
  trackSearch: $<HTMLInputElement>("track-search"),
  searchTitle: $("search-title"),
  kindTrack: $<HTMLButtonElement>("kind-track"),
  kindPodcast: $<HTMLButtonElement>("kind-podcast"),
  searchList: $<HTMLUListElement>("search-list"),
  searchError: $("search-error"),

  settings: $("settings"),
  closeSettings: $<HTMLButtonElement>("close-settings"),
  menuBarMode: $<HTMLInputElement>("menu-bar-mode"),
  alwaysOnTop: $<HTMLInputElement>("always-on-top"),
  miniFade: $<HTMLInputElement>("mini-fade"),
  settingsError: $("settings-error"),
};

const audio = new Audio();
audio.preload = "auto";

let current: Playable | null = null;
/** False for a track picked from search: the rotor never served it, so it
 *  gets no feedback, and the wave resumes once it ends. */
let fromWave = true;
/** Guards against double-reporting trackStarted when the element re-fires play. */
let reportedStart = false;
/** Set while advancing, so `ended` can't race a manual skip. */
let advancing = false;
/** Consecutive unplayable tracks — a whole bad batch must not spin forever. */
let failures = 0;
const MAX_FAILURES = 3;

/** Last settings seen from Rust. Mutated field-by-field so a write never
 *  clobbers a field this screen does not own (the chosen station, notably). */
let settings: Settings = {
  menuBarMode: false,
  alwaysOnTop: false,
  miniPlayer: false,
  miniFade: false,
  island: false,
  stationId: null,
  stationName: null,
};

const fmt = (sec: number) => {
  if (!isFinite(sec) || sec < 0) sec = 0;
  const m = Math.floor(sec / 60);
  const s = Math.floor(sec % 60);
  return `${m}:${s.toString().padStart(2, "0")}`;
};

const showError = (node: HTMLElement, msg: string | null) => {
  node.textContent = msg ?? "";
  node.classList.toggle("hidden", !msg);
};

/** Exactly one screen is visible at a time. */
function show(screen: HTMLElement) {
  for (const s of [el.login, el.player, el.stations, el.search, el.settings]) {
    s.classList.toggle("hidden", s !== screen);
  }
}

// ---- sign-in ----

async function startLogin() {
  showError(el.loginError, null);
  el.loginStart.disabled = true;
  try {
    const code = await api.authBegin();
    el.loginUrl.textContent = code.verification_url;
    el.loginUrl.href = code.verification_url;
    el.loginUserCode.textContent = code.user_code;
    el.loginCode.classList.remove("hidden");
    // Open in the real browser, not the webview — Yandex blocks embedded logins.
    openUrl(code.verification_url).catch(() => {});
    pollLogin(Math.max(code.interval, 1) * 1000);
  } catch (e) {
    el.loginStart.disabled = false;
    showError(el.loginError, String(e));
  }
}

function pollLogin(intervalMs: number) {
  const tick = async () => {
    try {
      const account = await api.authPoll();
      if (account) return enterPlayer(account);
      setTimeout(tick, intervalMs);
    } catch (e) {
      el.loginStart.disabled = false;
      el.loginCode.classList.add("hidden");
      showError(el.loginError, String(e));
    }
  };
  setTimeout(tick, intervalMs);
}

// ---- settings ----

/** Push one changed field, reverting the switch if Rust refuses it. */
async function saveSettings(patch: Partial<Settings>, revert: () => void) {
  const wanted = { ...settings, ...patch };
  try {
    applySettings(await api.settingsSet(wanted));
    showError(el.settingsError, null);
  } catch (e) {
    revert();
    showError(el.settingsError, String(e));
  }
}

/** Mirror the saved state into the UI. Rust owns the window itself. */
function applySettings(saved: Settings) {
  settings = saved;
  el.menuBarMode.checked = saved.menuBarMode;
  el.alwaysOnTop.checked = saved.alwaysOnTop;
  el.player.classList.toggle("mini", saved.miniPlayer);
  document.body.classList.toggle("mini-window", saved.miniPlayer);
  el.miniFade.checked = saved.miniFade;
  document.body.classList.toggle("mini-fade", saved.miniFade);
  el.player.classList.toggle("island", saved.island);
  document.body.classList.toggle("island-window", saved.island);
  if (saved.island) setIslandOpen(false);
  else document.body.classList.remove("island-open");
  el.stationName.textContent = saved.stationName ?? "Моя волна";
}

async function openSettings() {
  showError(el.settingsError, null);
  try {
    applySettings(await api.settingsGet());
  } catch (e) {
    showError(el.settingsError, String(e));
  }
  show(el.settings);
}

el.closeSettings.addEventListener("click", () => show(el.player));

el.menuBarMode.addEventListener("change", () => {
  const wanted = el.menuBarMode.checked;
  saveSettings({ menuBarMode: wanted }, () => (el.menuBarMode.checked = !wanted));
});

el.alwaysOnTop.addEventListener("change", () => {
  const wanted = el.alwaysOnTop.checked;
  saveSettings({ alwaysOnTop: wanted }, () => (el.alwaysOnTop.checked = !wanted));
});

el.miniFade.addEventListener("change", () => {
  const wanted = el.miniFade.checked;
  saveSettings({ miniFade: wanted }, () => (el.miniFade.checked = !wanted));
});

// The ⋯ menu is native; its choices come back as `menu:main` events.
el.openMenu.addEventListener("click", () => {
  const r = el.openMenu.getBoundingClientRect();
  api.mainMenu(r.left, r.bottom + 4).catch((e) => console.error("menu failed:", e));
});

listen<string>("menu:main", ({ payload }) => {
  switch (payload) {
    case "main-settings":
      return openSettings();
    case "main-logout":
      return signOut();
  }
});

// ---- window buttons ----

// The app draws its own title-bar buttons, the same on every platform; one
// template is stamped into each screen's bar.
const windowControls = document.querySelector<HTMLTemplateElement>("#window-controls")!;
for (const slot of document.querySelectorAll(".window-controls")) {
  slot.append(windowControls.content.cloneNode(true));
}

/** Fold into the mini player or the island; the other screens fit neither. */
function compact(patch: Partial<Settings>) {
  saveSettings(patch, () => {});
  show(el.player);
}

document.addEventListener("click", (e) => {
  const button = (e.target as HTMLElement | null)?.closest?.<HTMLElement>("[data-window]");
  switch (button?.dataset.window) {
    case "close":
      // Goes through CloseRequested, so menu-bar mode still hides instead.
      return getCurrentWindow().close().catch((err) => console.error("close failed:", err));
    case "mini":
      return compact({ miniPlayer: true, island: false });
    case "island":
      return compact({ island: true, miniPlayer: false });
  }
});

document.addEventListener("contextmenu", (e) => {
  if (!settings.miniPlayer && !settings.island) return;
  e.preventDefault();
  api.miniMenu().catch((err) => console.error("mini menu failed:", err));
});

listen("menu:expand", () => leaveMini());

// The fade itself is CSS; this only tracks whether the window is focused.
document.body.classList.toggle("inactive", !document.hasFocus());
getCurrentWindow()
  .onFocusChanged(({ payload: focused }) => {
    document.body.classList.toggle("inactive", !focused);
    // A click anywhere else folds the island back into the notch.
    if (!focused && settings.island) setIslandOpen(false);
  })
  .catch(() => {});

// ---- island ----

/** Resize first when growing and last when shrinking, so the content never
 *  lays out for a window size it does not have yet. */
async function setIslandOpen(open: boolean) {
  const body = document.body.classList;
  if (!open) body.remove("island-open");
  let geometry: IslandGeometry | null = null;
  try {
    geometry = await api.islandResize(open);
  } catch (e) {
    console.warn("island resize failed", e);
  }
  if (!settings.island) return;
  if (geometry) {
    const root = document.documentElement;
    root.dataset.island = geometry.notch ? "notch" : "pill";
    root.style.setProperty("--notch-w", `${geometry.notchWidth}px`);
    root.style.setProperty("--notch-h", `${geometry.notchHeight}px`);
  }
  if (open) body.add("island-open");
}

// Collapsed, a click opens it; open, a click on anything but a control closes it.
el.player.addEventListener("click", (e) => {
  if (!settings.island) return;
  const target = e.target as HTMLElement | null;
  if (target?.closest?.("button, .track")) return;
  setIslandOpen(!document.body.classList.contains("island-open"));
});

// The island is pinned to the notch, so it must not start a window drag.
window.addEventListener(
  "mousedown",
  (e) => {
    const target = e.target as HTMLElement | null;
    if (settings.island && !target?.closest?.("button, .track")) {
      e.stopImmediatePropagation();
    }
  },
  true,
);

/** Tauri's own drag-region rules: "deep" covers the subtree, "false" and
 *  unmarked controls block it, a bare attribute only its own element. */
function isDragRegion(path: EventTarget[]): boolean {
  for (const node of path) {
    if (!(node instanceof HTMLElement)) continue;
    const attr = node.getAttribute("data-tauri-drag-region");
    const clickable =
      /^(A|BUTTON|INPUT|SELECT|TEXTAREA|LABEL|SUMMARY)$/.test(node.tagName) ||
      (node.hasAttribute("contenteditable") && node.getAttribute("contenteditable") !== "false") ||
      (node.hasAttribute("tabindex") && node.getAttribute("tabindex") !== "-1") ||
      /^(button|link|menuitem|tab|checkbox|radio|switch|option)$/.test(node.getAttribute("role") ?? "");
    if (attr === null) {
      if (clickable) return false;
      continue;
    }
    if (attr === "false") return false;
    if (attr === "deep") return true;
    return node === path[0];
  }
  return false;
}

// Tauri's drag makes the window jump on an activating click, so drags go
// through `window_drag` instead; double clicks still reach Tauri's handler.
window.addEventListener(
  "mousedown",
  (e) => {
    if (e.button !== 0 || e.detail !== 1 || !isDragRegion(e.composedPath())) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    api.windowDrag().catch(() => {});
  },
  true,
);

el.islandFull.addEventListener("click", () => leaveMini());

// ---- waves ----

/** The whole catalogue, fetched once; ~700 entries, so the list is filtered. */
let allStations: Station[] = [];
const MAX_ROWS = 120;

el.openStations.addEventListener("click", async () => {
  show(el.stations);
  showError(el.stationsError, null);
  if (!allStations.length) {
    el.stationList.replaceChildren(hint("Loading waves…"));
    try {
      allStations = await api.stations();
    } catch (e) {
      showError(el.stationsError, String(e));
      el.stationList.replaceChildren();
      return;
    }
  }
  renderStations();
  el.stationSearch.focus();
});

el.closeStations.addEventListener("click", () => show(el.player));
el.stationSearch.addEventListener("input", renderStations);

function hint(text: string) {
  const li = document.createElement("li");
  li.className = "queue-empty";
  li.textContent = text;
  return li;
}

function renderStations() {
  const q = el.stationSearch.value.trim().toLowerCase();
  const matches = q
    ? allStations.filter(
        (s) => s.name.toLowerCase().includes(q) || s.id.toLowerCase().includes(q),
      )
    : allStations;

  el.stationList.replaceChildren();
  if (!matches.length) {
    el.stationList.append(hint("No wave matches that."));
    return;
  }

  let group = "";
  for (const s of matches.slice(0, MAX_ROWS)) {
    const band = s.featured ? "For you" : "All waves";
    if (band !== group) {
      group = band;
      const head = document.createElement("li");
      head.className = "station-group muted small";
      head.textContent = band;
      el.stationList.append(head);
    }
    el.stationList.append(stationRow(s));
  }

  if (matches.length > MAX_ROWS) {
    el.stationList.append(
      hint(`${matches.length - MAX_ROWS} more — narrow the search to see them.`),
    );
  }
}

function stationRow(s: Station) {
  const li = document.createElement("li");
  li.className = "station-item";
  li.dataset.tauriDragRegion = "false"; // a row is a click target, not a handle
  if (s.id === (settings.stationId ?? "user:onyourwave")) li.classList.add("on");
  li.title = s.id;

  const art = document.createElement("span");
  art.className = "station-art";
  if (s.color) art.style.background = s.color;
  if (s.iconUrl) {
    const img = document.createElement("img");
    img.alt = "";
    img.loading = "lazy";
    img.src = s.iconUrl;
    art.append(img);
  }

  const name = document.createElement("span");
  name.className = "station-title";
  name.textContent = s.name;

  const kind = document.createElement("span");
  kind.className = "station-kind muted";
  kind.textContent = s.kind;

  li.addEventListener("click", () => pickStation(s));
  li.append(art, name, kind);
  return li;
}

/** Switch waves. Feedback for the outgoing track has to go to the station it
 *  was played on, so it is sent before the session is replaced. */
async function pickStation(s: Station) {
  if (s.id === (settings.stationId ?? "user:onyourwave")) return show(el.player);
  // A switch mid-`advance` would hand the old station's track to the new
  // session, so wait for it to land first.
  if (advancing) return;

  if (current && reportedStart && fromWave) {
    // Awaited, not fired off: the rotor session is replaced below, and a skip
    // that arrives after the swap is charged to the wrong station.
    await api
      .waveFeedback("skip", current.track.id, audio.currentTime)
      .catch((e) => console.warn("feedback failed", e));
  }
  audio.pause();
  current = null; // so `advance` reports nothing against the new station
  reportedStart = false;

  show(el.player);
  el.title.textContent = "—";
  el.artist.textContent = "—";
  el.queueList.replaceChildren(hint("Loading the wave…"));

  try {
    applySettings(await api.waveSetStation(s.id, s.name));
  } catch (e) {
    showError(el.playerError, String(e));
    return;
  }
  await advance();
}

// ---- track search ----

el.openSearch.addEventListener("click", () => {
  show(el.search);
  showError(el.searchError, null);
  el.trackSearch.focus();
  el.trackSearch.select();
});

// Back steps out of an open podcast first, then out of search.
el.closeSearch.addEventListener("click", () => {
  if (!podcast) return show(el.player);
  podcast = null;
  renderResults();
});

type SearchKind = "track" | "podcast";
let searchKind: SearchKind = "track";
let searchTimer = 0;
/** Bumped per query, so a slow reply cannot overwrite a newer one. */
let searchSeq = 0;
/** The last result list, kept so leaving a podcast does not search again. */
let results: Track[] | Podcast[] | null = null;
/** The podcast whose episodes are listed, if one is open. */
let podcast: Podcast | null = null;

el.trackSearch.addEventListener("input", () => {
  clearTimeout(searchTimer);
  searchTimer = window.setTimeout(runSearch, 300);
});

function setKind(kind: SearchKind) {
  if (kind === searchKind) return;
  searchKind = kind;
  el.kindTrack.classList.toggle("on", kind === "track");
  el.kindPodcast.classList.toggle("on", kind === "podcast");
  el.trackSearch.placeholder = kind === "track" ? "Search tracks…" : "Search podcasts…";
  el.trackSearch.focus();
  runSearch();
}

el.kindTrack.addEventListener("click", () => setKind("track"));
el.kindPodcast.addEventListener("click", () => setKind("podcast"));

async function runSearch() {
  const text = el.trackSearch.value.trim();
  const seq = ++searchSeq;
  const kind = searchKind;
  podcast = null;
  results = null;
  showError(el.searchError, null);
  if (!text) return renderResults();
  el.searchTitle.textContent = "Search";
  el.searchList.replaceChildren(hint("Searching…"));
  try {
    const found =
      kind === "track" ? await api.searchTracks(text) : await api.searchPodcasts(text);
    if (seq !== searchSeq) return;
    results = found;
    renderResults();
  } catch (e) {
    if (seq !== searchSeq) return;
    el.searchList.replaceChildren();
    showError(el.searchError, String(e));
  }
}

function renderResults() {
  el.searchTitle.textContent = "Search";
  if (!results) return el.searchList.replaceChildren();
  const rows =
    searchKind === "track"
      ? (results as Track[]).map((t) => trackRow(t, t.artist))
      : (results as Podcast[]).map(podcastRow);
  el.searchList.replaceChildren(...(rows.length ? rows : [hint("Nothing found.")]));
}

/** One list row: art, two lines of text, and a trailing note. */
function row(artUrl: string | null, line1: string, line2: string, note: string) {
  const li = document.createElement("li");
  li.className = "queue-item";
  li.dataset.tauriDragRegion = "false"; // a row is a click target, not a handle
  li.title = `${line1} — ${line2}`;

  const art = document.createElement("img");
  art.className = "queue-art";
  art.alt = "";
  if (artUrl) art.src = artUrl;

  const meta = document.createElement("div");
  meta.className = "queue-meta";
  const title = document.createElement("div");
  title.className = "queue-title";
  title.textContent = line1;
  const sub = document.createElement("div");
  sub.className = "queue-artist";
  sub.textContent = line2;
  meta.append(title, sub);

  const time = document.createElement("span");
  time.className = "queue-time";
  time.textContent = note;

  li.append(art, meta, time);
  return li;
}

function trackRow(t: Track, subtitle: string) {
  const li = row(t.coverThumbUrl, t.title, subtitle, fmt(t.durationMs / 1000));
  if (!t.available) li.classList.add("unavailable");
  li.addEventListener("click", () => playSearched(t));
  return li;
}

function podcastRow(p: Podcast) {
  const episodes = `${p.episodeCount} episode${p.episodeCount === 1 ? "" : "s"}`;
  const li = row(p.coverThumbUrl, p.title, episodes, "");
  li.addEventListener("click", () => openPodcast(p));
  return li;
}

const dateFmt = new Intl.DateTimeFormat(undefined, {
  day: "numeric",
  month: "short",
  year: "numeric",
});

/** List a podcast's episodes, newest first, in place of the results. */
async function openPodcast(p: Podcast) {
  const seq = ++searchSeq;
  podcast = p;
  el.searchTitle.textContent = p.title;
  showError(el.searchError, null);
  el.searchList.replaceChildren(hint("Loading episodes…"));
  try {
    const episodes = await api.podcastEpisodes(p.id);
    if (seq !== searchSeq || podcast !== p) return;
    const rows = episodes.map((t) =>
      trackRow(t, t.pubDate ? dateFmt.format(new Date(t.pubDate)) : p.title),
    );
    el.searchList.replaceChildren(...(rows.length ? rows : [hint("No episodes yet.")]));
    el.searchList.scrollTop = 0;
  } catch (e) {
    if (seq !== searchSeq) return;
    el.searchList.replaceChildren();
    showError(el.searchError, String(e));
  }
}

/** Play a search result now; the wave picks up again after it. */
async function playSearched(t: Track) {
  if (advancing) return;
  advancing = true;
  show(el.player);
  showError(el.playerError, null);
  // The wave track being cut off counts as skipped, like pressing next.
  if (current && reportedStart && fromWave) {
    api
      .waveFeedback("skip", current.track.id, audio.currentTime)
      .catch((e) => console.warn("feedback failed", e));
  }
  try {
    const item = await api.trackPlay(t);
    current = item;
    fromWave = false;
    reportedStart = false;
    failures = 0;
    render(item);
    audio.src = item.url;
    await audio.play();
  } catch (e) {
    showError(el.playerError, String(e));
  } finally {
    advancing = false;
  }
}

// The webview's own context menu only offers Reload and friends, which this
// app has no use for. Text fields keep theirs — that one is cut/copy/paste.
document.addEventListener("contextmenu", (e) => {
  const target = e.target as HTMLElement | null;
  if (!target?.closest?.("input, textarea")) e.preventDefault();
});

// macOS zooms a window when its title bar is double-clicked, and Tauri's drag
// region forwards that as a maximize. This window has one size per mode, so the
// double click is swallowed before the drag handler on `document` sees it.
window.addEventListener(
  "mouseup",
  (e) => {
    const target = e.target as HTMLElement | null;
    if (e.detail >= 2 && target?.closest?.("[data-tauri-drag-region]")) {
      e.stopImmediatePropagation();
    }
    // In the mini player the same double click opens the full player.
    if (e.detail === 2 && settings.miniPlayer && !target?.closest?.("button, .track")) {
      leaveMini();
    }
  },
  true,
);

// ---- tray ----

/** Play, recovering when the webview has no user activation yet. */
async function tryPlay() {
  try {
    await audio.play();
  } catch {
    // WebKit only allows programmatic playback after a real click in the page,
    // so a tray click alone cannot start audio from cold.
    await api.showWindow().catch(() => {});
    showError(el.playerError, "Press ▶ in the window once to allow playback.");
  }
}

listen("tray:play-pause", () => {
  if (audio.paused) tryPlay();
  else audio.pause();
});

listen("tray:next", () => advance("skip"));

// ---- player ----

async function enterPlayer(account: Account) {
  el.who.textContent = account.hasPlus
    ? account.displayName
    : `${account.displayName} · no Plus`;
  try {
    applySettings(await api.settingsGet());
  } catch (e) {
    console.warn("settings unavailable", e);
  }
  show(el.player);
  // Load the first track but leave it paused until the user presses play.
  await advance(null, 0, false);
}

/** Report how the current track ended, then load the next one. */
async function advance(
  reason: "trackFinished" | "skip" | null = null,
  skipAhead = 0,
  play = true,
) {
  if (advancing) return;
  advancing = true;
  el.next.disabled = true;
  showError(el.playerError, null);

  try {
    if (reason && current && fromWave) {
      // Feedback drives the wave; a failure here must not stop playback.
      api
        .waveFeedback(reason, current.track.id, audio.currentTime)
        .catch((e) => console.warn("feedback failed", e));
    }

    const item = skipAhead > 0 ? await api.waveSkipTo(skipAhead) : await api.waveNext();
    if (!item) {
      showError(el.playerError, "The wave returned no tracks.");
      return;
    }

    current = item;
    fromWave = true;
    reportedStart = false;
    failures = 0;
    render(item);
    audio.src = item.url;
    if (play) await audio.play();
  } catch (e) {
    showError(el.playerError, String(e));
  } finally {
    advancing = false;
    el.next.disabled = false;
    refreshQueue();
  }
}

/** Render what the wave has buffered ahead of the current track. */
async function refreshQueue() {
  let upcoming: Track[] = [];
  try {
    upcoming = await api.waveQueue();
  } catch {
    // A queue we cannot read is not worth interrupting playback for.
  }

  el.queueList.replaceChildren();
  if (!upcoming.length) {
    el.queueList.append(hint("Loading the wave…"));
    return;
  }

  upcoming.forEach((t, i) => {
    const li = document.createElement("li");
    li.className = "queue-item";
    li.dataset.tauriDragRegion = "false"; // a row is a click target, not a handle
    li.title = `${t.artist} — ${t.title}`;

    const index = document.createElement("span");
    index.className = "queue-index";
    index.textContent = String(i + 1);

    const art = document.createElement("img");
    art.className = "queue-art";
    art.alt = "";
    if (t.coverThumbUrl) art.src = t.coverThumbUrl;

    const meta = document.createElement("div");
    meta.className = "queue-meta";
    const title = document.createElement("div");
    title.className = "queue-title";
    title.textContent = t.title;
    const artist = document.createElement("div");
    artist.className = "queue-artist";
    artist.textContent = t.artist;
    meta.append(title, artist);

    const time = document.createElement("span");
    time.className = "queue-time";
    time.textContent = fmt(t.durationMs / 1000);

    // Clicking track i means dropping the i tracks queued before it.
    li.addEventListener("click", () => advance("skip", i));

    li.append(index, art, meta, time);
    el.queueList.append(li);
  });
}

function render(item: Playable) {
  const t = item.track;
  el.title.textContent = t.title;
  el.artist.textContent = t.artist;
  el.total.textContent = fmt(t.durationMs / 1000);
  el.like.classList.remove("on");
  el.like.disabled = false;
  if (t.coverUrl) el.cover.src = t.coverUrl;
  else el.cover.removeAttribute("src");
  paint(0, t.durationMs / 1000);

  // Wire up the OS media keys / Now Playing panel.
  if ("mediaSession" in navigator) {
    navigator.mediaSession.metadata = new MediaMetadata({
      title: t.title,
      artist: t.artist,
      album: t.album,
      artwork: t.coverUrl ? [{ src: t.coverUrl, sizes: "400x400", type: "image/jpeg" }] : [],
    });
    navigator.mediaSession.setActionHandler("play", () => audio.play());
    navigator.mediaSession.setActionHandler("pause", () => audio.pause());
    navigator.mediaSession.setActionHandler("nexttrack", () => advance("skip"));
  }
}

// ---- seeking ----

/** The element's duration is authoritative once known; before that the wave's
 *  metadata is all there is. */
const duration = () =>
  isFinite(audio.duration) && audio.duration > 0
    ? audio.duration
    : current
      ? current.track.durationMs / 1000
      : 0;

/** Set while a pointer is down on the bar, so `timeupdate` stops fighting it. */
let scrubbing = false;

function paint(at: number, total = duration()) {
  const ratio = total ? Math.min(1, Math.max(0, at / total)) : 0;
  el.bar.style.width = `${ratio * 100}%`;
  el.elapsed.textContent = fmt(at);
  el.seek.setAttribute("aria-valuemax", String(Math.round(total)));
  el.seek.setAttribute("aria-valuenow", String(Math.round(at)));
}

/** Where along the track a pointer at `clientX` points. */
function positionAt(clientX: number) {
  const rect = el.seek.getBoundingClientRect();
  if (!rect.width) return null;
  const total = duration();
  if (!total) return null;
  const ratio = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
  return ratio * total;
}

el.seek.addEventListener("pointerdown", (e) => {
  const at = positionAt(e.clientX);
  if (at === null) return;
  scrubbing = true;
  el.seek.setPointerCapture(e.pointerId);
  el.seek.classList.add("scrubbing");
  paint(at);
  e.preventDefault();
});

el.seek.addEventListener("pointermove", (e) => {
  if (!scrubbing) return;
  const at = positionAt(e.clientX);
  if (at !== null) paint(at);
});

const endScrub = (e: PointerEvent) => {
  if (!scrubbing) return;
  scrubbing = false;
  el.seek.classList.remove("scrubbing");
  if (el.seek.hasPointerCapture(e.pointerId)) el.seek.releasePointerCapture(e.pointerId);
  const at = positionAt(e.clientX);
  if (at !== null) audio.currentTime = at;
};

el.seek.addEventListener("pointerup", endScrub);
el.seek.addEventListener("pointercancel", endScrub);

// Arrow keys nudge, for when the bar is focused.
el.seek.addEventListener("keydown", (e) => {
  const step = e.key === "ArrowLeft" ? -5 : e.key === "ArrowRight" ? 5 : 0;
  if (!step || !duration()) return;
  audio.currentTime = Math.min(duration(), Math.max(0, audio.currentTime + step));
  e.preventDefault();
});

// ---- audio element ----

/** The button's label is a drawn shape, so it is swapped by class. */
function showPlaying(playing: boolean) {
  el.playGlyph.className = `glyph ${playing ? "pause" : "play"}`;
  el.play.setAttribute("aria-label", playing ? "Pause" : "Play");
  document.body.classList.toggle("playing", playing);
}

audio.addEventListener("playing", () => {
  showPlaying(true);
  if (!reportedStart && current) {
    reportedStart = true;
    if (fromWave) {
      api.waveFeedback("trackStarted", current.track.id).catch((e) =>
        console.warn("feedback failed", e),
      );
    }
  }
});

audio.addEventListener("pause", () => {
  showPlaying(false);
});

audio.addEventListener("loadedmetadata", () => {
  el.total.textContent = fmt(duration());
});

audio.addEventListener("timeupdate", () => {
  if (scrubbing) return;
  paint(audio.currentTime);
});

audio.addEventListener("ended", () => {
  // A failed stream can fire `ended` as well as `error`. Only a track that
  // actually started playing can have finished.
  if (!reportedStart) return;
  advance("trackFinished");
});

audio.addEventListener("error", () => {
  // Expired or rejected stream URL. Tell the rotor, then move on — but stop
  // after a few in a row rather than spinning through an unplayable batch.
  if (++failures > MAX_FAILURES) {
    showError(el.playerError, "Several tracks in a row could not be played. Check your Plus subscription.");
    return;
  }
  showError(el.playerError, "Could not play that track, skipping…");
  advance("skip");
});

el.play.addEventListener("click", () => {
  if (audio.paused) tryPlay();
  else audio.pause();
});

el.next.addEventListener("click", () => advance("skip"));

el.like.addEventListener("click", async () => {
  if (!current) return;
  el.like.disabled = true;
  try {
    await api.likeTrack(current.track.id, true);
    el.like.classList.add("on");
  } catch (e) {
    el.like.disabled = false;
    showError(el.playerError, String(e));
  }
});

async function signOut() {
  audio.pause();
  audio.removeAttribute("src");
  current = null;
  allStations = []; // the dashboard half of it is per-account
  await api.authLogout();
  await leaveMini();
  show(el.login);
  el.loginCode.classList.add("hidden");
  el.loginStart.disabled = false;
}

el.loginStart.addEventListener("click", startLogin);

/** Back to the full player from the mini player or the island; sign-in
 *  fits neither, so returning to it goes through here first. */
async function leaveMini() {
  if (!settings.miniPlayer && !settings.island) return;
  await saveSettings({ miniPlayer: false, island: false }, () => {});
}

// ---- boot ----

api
  .authRestore()
  .then(async (account) => {
    if (account) return enterPlayer(account);
    applySettings(await api.settingsGet().catch(() => settings));
    await leaveMini();
  })
  .catch((e) => showError(el.loginError, String(e)));
