# Open-source Yandex Music player — feasibility & technical analysis

**Date:** 2026-09-22
**Scope:** how a third-party client can (a) authenticate a user, (b) play the catalogue, (c) support **Моя волна / My Vibe** — and what that costs in risk and maintenance.

**Evidence legend** — every technical claim below is tagged:

| Tag | Meaning |
|---|---|
| **[V]** | Verified in this session — read out of the live web bundle `v4.1626.1` or observed on `music.yandex.ru` |
| **[S]** | From source — open-source repos or library documentation, not independently re-verified |
| **[?]** | Unverified — needs a check before you build on it |

---

## 0. TL;DR

1. **There is no sanctioned third-party API and no way to register your own OAuth app.** [S] Every working client borrows the credentials of Yandex's *own* official app. That is the single decision that colours the whole project.
2. **Auth is a solved problem in practice:** OAuth **device-code flow** against `oauth.yandex.ru` using the well-known Yandex Music `client_id`. Good UX, returns a `refresh_token`, no password ever touches your app.
3. **Playback has two independent paths** — the modern *web* path (`get-file-info` + HMAC signature) and the older *mobile* path (`download-info` + MD5 salt). Don't conflate them. Pick one, know why.
4. **The web path's signing key rotates.** The key published by `yt-dlp-yandex-music-plugin` is pinned to bundle `v4.1603.1`; the live bundle today is **`v4.1626.1`** [V]. Any client hardcoding that key is on a timer.
5. **A pure browser SPA is impossible** — CORS. "Simple, clean player" therefore means desktop app, native app, browser extension, or thin local proxy. See §2.
6. **Моя волна is not a playlist** — it's a stateful rotor session that *requires* you to send feedback events. Skip the feedback and the wave degrades into noise.

---

## 1. The ground truth: no third-party API

Yandex does not publish a public Music API and does not let you create an OAuth application with music scopes. The documentation of the de-facto reference library states this plainly: custom OAuth applications cannot be created. [S]

What every existing client does instead is reuse the official app's `client_id`:

```
client_id = 23cabbbdc6cd418abb4b39c32c41195d
```
[S] — the Yandex Music application's own identifier. No `client_secret` is required for the flows below. [S] [?] *verify empirically — Yandex's generic OAuth docs describe `client_secret` as required for confidential clients*

**Consequences you should accept up front:**

- Your users grant "Yandex Music" access, not "YourPlayer" access. The consent screen will not carry your name. This is confusing for users and worth explaining in your README.
- Yandex can invalidate this path at any time, for everyone, without notice.
- You are in ToS grey territory. See §10.

---

## 2. The constraint nobody mentions first: CORS

**[V] — verified this session.** Both API hosts run a strict origin allowlist containing exactly one entry: the official web player.

```console
$ curl -sI -X OPTIONS -H "Origin: https://music.yandex.ru" \
       -H "Access-Control-Request-Method: GET" https://api.music.yandex.ru/get-file-info
HTTP/2 200
access-control-allow-origin: https://music.yandex.ru      # ← echoed only for this origin
access-control-allow-credentials: true
access-control-max-age: 86400

$ curl -sI -H "Origin: https://example.com" https://api.music.yandex.net/account/status
HTTP/2 403
vary: Access-Control-Request-Method                        # ← origin-aware, no ACAO returned
```

Any other origin gets **no** `Access-Control-Allow-Origin` header at all, so the browser blocks the response. A static web app served from your own domain cannot call this API, full stop. This is not something you can work around with cleverness — it is the server's decision, and it is deliberate.

So "simple, clean, open source player" must resolve to one of:

| Shape | CORS | Token storage | Notes |
|---|---|---|---|
| **Desktop app (Tauri / Electron)** | N/A — native HTTP | keychain or file | **Recommended.** Tauri gives you a small binary and a real HTTP client that ignores CORS. |
| **Native (Kotlin / Swift / Rust)** | N/A | keychain or file | Best UX, most work per platform. |
| **Browser extension** | Bypassed via `host_permissions` | `chrome.storage` | Can piggyback on the user's existing `Session_id` cookie. Store-review risk. |
| **Web UI + local proxy** | Proxy adds headers | Server-side | Two moving parts; fine for self-hosters, poor for "download and run". |
| **Pure static SPA** | ❌ **impossible** | — | Rules this out entirely. |

**Recommendation: Tauri.** You get a web UI (the "clean" part), a Rust HTTP layer that sidesteps CORS, and ~10 MB binaries.

---

## 3. Authentication

### Option A — OAuth device-code flow ⭐ recommended

The best UX available to an open-source client. The user never types a password into your app, and you get a refresh token.

```
POST https://oauth.yandex.ru/device/code
     client_id=23cabbbdc6cd418abb4b39c32c41195d
     device_id=<stable random id you generate>
     device_name=<"YourPlayer on andrey-mac">
  → { device_code, user_code, verification_url, interval, expires_in }
```

Show the user `user_code` and `verification_url` (optionally as a QR code), then poll:

```
POST https://oauth.yandex.ru/token
     grant_type=device_code
     code=<device_code>
     client_id=...
  → 400 {"error":"authorization_pending"}   … keep polling every `interval` s
  → 200 { access_token, refresh_token, expires_in, token_type }
```
[S]

⚠️ **Correction — `client_secret` IS required.** An earlier draft of this document said it was not, based on a summary that merely failed to mention it. The reference library's source passes one on the `/token` exchange (`_DEFAULT_CLIENT_SECRET`), alongside the `client_id`. **[V]** — read from `yandex_music/_client/device_auth.py`. Both values are published constants of the official Android app. `/device/code` takes only the `client_id`; `/token` takes both.

`device_id` / `device_name` are standard Yandex OAuth device-flow parameters [S]; `expires_in` is typically ~1 year [S]. **Token lifecycle is entirely your responsibility** — the reference library explicitly does not persist or auto-refresh. [S] Build refresh-on-401 into your HTTP layer from day one.

### Option B — implicit OAuth intercepted in a webview

```
https://oauth.yandex.ru/authorize?response_type=token&client_id=23cabbbdc6cd418abb4b39c32c41195d
```

After consent the browser lands on `https://music.yandex.ru/#access_token=…&token_type=bearer&expires_in=…` — you read the token out of the URL fragment. [S] This is what the Chrome/Firefox "Yandex Music Token" extensions do. [S]

In a desktop app: open this in an embedded webview and intercept the navigation to the redirect. Downside: **no refresh token** — when it expires the user re-authenticates from scratch.

### Option C — "paste your token" fallback

Always ship this. It costs you a text input and rescues every user whose flow breaks. Point them at the existing open-source helpers (`ym-token.marshal.dev`, the Android APK, the browser extensions). [S]

### Option D — web `Session_id` cookie

The web player authenticates with the `Session_id` cookie scoped to `.yandex.ru`, which automatically covers the `api.music.yandex.ru` subdomain. [S] Only viable for a browser extension. Fragile, rotates, and ties you to the web API generation.

### Storage

Two options, and the trade-off is real.

**The OS keychain** (`keyring` crate in Rust) is stronger in principle: access is gated per-application, so another process running as the same user cannot silently read the token.

**But on macOS it is painful for an unsigned app.** The keychain binds an item's ACL to the binary's *code signature*. A `cargo build` binary is ad-hoc signed with no team identifier, so its designated requirement is effectively "this exact code hash" — and every rebuild produces a new one. **[V]** *verified: a no-op rebuild changed `CDHash` from `a246a795…` to `6b3486ec…`.* The keychain therefore sees a different application each time and re-prompts; "Always Allow" only holds until the next compile. This resolves only once the app ships with a stable signing identity (Developer ID or a self-signed cert).

**This project stores the token in a mode-0600 file** under the app config dir. Be clear-eyed about what that costs: any process running as your user can read it, which the keychain would have prevented. If you later sign the app properly, moving to the keychain is a contained change — `store.rs` is the only module that touches persistence.

Either way, treat the token as a full account credential — it is one, and it is revocable only at [yandex.ru/security/apps](https://yandex.ru/security/apps).

---

## 4. Two API generations — know which you're on

| | **Mobile API** | **Web API** |
|---|---|---|
| Host | `api.music.yandex.net` [S] | `api.music.yandex.ru` [S] |
| Auth | `Authorization: OAuth <token>` | `Session_id` cookie, or the same OAuth header |
| Track URL | `tracks/{id}/download-info` → XML → MD5 salt | `get-file-info` → HMAC-SHA256 sign |
| Stability | Unchanged for years [S] | Key rotates per frontend release [V] |
| Client header | `X-Yandex-Music-Client: YandexMusicAndroid/<build>` [S] | browser headers |

They are largely the same backend — the difference is the *stream-authorisation* mechanism, and that difference is the whole engineering story.

> **Why the hosts are [S] and not [V]:** the web frontend is a Next.js App Router app that resolves data server-side via RSC, so browser network capture surfaced only static assets and telemetry — no API XHRs. The bundle itself calls `httpClient.get("get-file-info")` with a **relative** path against a base URL held in config I did not locate. Both host names come from third-party clients, not from my own observation.

> **A retired third generation:** an older web API at `handlers/*.jsx` **no longer exists**. [S] yt-dlp's built-in Yandex extractor still calls it and crashes for exactly that reason. [S] If you find a tutorial using `handlers/`, it is dead — ignore it. Note this is *not* evidence against `download-info`, which is a different API.

Common endpoints (mobile generation) [S]:

```
GET  /account/status                     → has Plus? region? restrictions?
GET  /users/{uid}/playlists/list
GET  /users/{uid}/likes/tracks
GET  /tracks?trackIds=1,2,3              (POST for large batches)
GET  /albums/{id}/with-tracks
GET  /artists/{id}/brief-info
GET  /search?text=…&type=all&page=0
POST /users/{uid}/likes/tracks/add-multiple
GET  /landing3?blocks=personalplaylists,…
```

Check `/account/status` on startup: **a free account gets previews/ads, not full tracks.** Your player must degrade gracefully or tell the user plainly.

---

## 5. Playback

### 5.1 The web path — `get-file-info` (verified this session)

Read directly out of the live bundle `v4.1626.1` [V]:

```js
// chunk 3254 — the resource layer
async getFileInfo(t, e) {
  let a = await this.httpClient.get("get-file-info", this.createHttpOptions({
    timeoutKey: "getFileInfo",
    searchParams: { ts, trackId, quality, codecs, transports, sign, fromPromoLanding },
  }));
  ...
}
// also: "get-file-info/batch" for prefetching several tracks at once
```

**The signature.** Also [V], from chunk 81170:

```js
u = "".concat(ts).concat(trackId).concat(quality).concat(codecs.join("")).concat(transports.join(""));
sign = await this.tools.createSign({ data: u, secretKey: this.secretKey });
```

So — **the concatenation order is [V]** (read above); **the hash and encoding are [S]**, from `yt-dlp-yandex-music-plugin`. I found the `createSign` *call sites* in the bundle but not its implementation, so HMAC-SHA256 + base64 + strip-padding is source-tier:

```
message = ts + trackId + quality + codecs.join("") + transports.join("")    # [V]
sign    = base64( HMAC_SHA256(secretKey, message) ).replace(/=+$/, "")      # [S]
```

⚠️ **The trap:** codecs and transports are joined **without separators for the signature**, but appear **comma-separated in the query string**. Get this wrong and every request 403s.

```js
const ts = Math.floor(Date.now() / 1000);
const msg = `${ts}${trackId}${quality}${codecs.join('')}${transports.join('')}`;
const k = await crypto.subtle.importKey('raw', enc.encode(SECRET),
            { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
const sign = btoa(String.fromCharCode(...new Uint8Array(
              await crypto.subtle.sign('HMAC', k, enc.encode(msg))))).replace(/=+$/, '');
// query: ?ts=…&trackId=…&quality=…&codecs=flac,aac&transports=raw&sign=…
```

**Parameter values:**

- `quality`: `lossless` | `nq` | `lq` — **[V]**, all three string literals present in the bundle
- `transports`: `raw` | `encraw` | `enclaw` — **[V]**, all three present
- `codecs`: `flac`, `mp3` — **[S]** (yt-dlp plugin). I never grepped codec names; `aac` is plausible but **[?]**
- The server silently falls back to the best quality it actually holds — **[S]**. There is no "what qualities does this track have" endpoint: you ask for the best and take what arrives. [S]

**The secret key.** It lives in a per-platform config object — the bundle resolves it as `player.secretKey.web` [V], meaning there are sibling keys for the other platforms. It is **not** a constant of the protocol; it ships with the frontend release. The value published by `yt-dlp-yandex-music-plugin` is tagged to `v4.1603.1` [S]; production today serves **`v4.1626.1`** [V]. Rotation is real and observable.

*(I deliberately did not extract the current key value — it would be stale by the time you read this, which is precisely the point being made.)*

**Therefore, if you take the web path, do not hardcode the key.** Resolve it at runtime:

1. `GET https://music.yandex.ru/` → regex the `music/v4.X.Y/` version out of the script URLs
2. Fetch the chunk containing `player.secretKey` and extract it
3. Cache per version, re-scrape on the first 403

That is a scraper inside your player, and it will break. Budget for it.

### 5.2 The mobile path — `download-info`

```
GET /tracks/{trackId}/download-info        (Authorization: OAuth …)
  → list of variants (codec, bitrate, downloadInfoUrl)
GET <downloadInfoUrl>                       → XML: <host><path><ts><s>
```

You then build the final URL from those XML fields using an MD5 over a **hardcoded salt + path + s**. [S] The salt value is a long-published constant in the reference library — take it from `MarshalX/yandex-music-api` rather than from memory. [?] *I did not verify the salt value in this session; read it from the library source.*

**This path has been stable for years and needs no runtime scraping.** [S]

### 5.3 Transports and DRM

- `raw` — unencrypted URLs from `strm.yandex.net`, playable directly. [S]
- `encraw` / `enclaw` — encrypted payloads requiring client-side AES decryption. The bundle contains an explicit *"retry enclaw request"* error path [V], i.e. the player falls back between transports.

At least one production client (the Music Assistant provider) implements lossless FLAC "with AES decryption for the encraw transport". [S]

**Judgement call:** request `transports=raw` first and simply fail over to a lower quality if the server refuses. Implementing decryption for `encraw` is where "unofficial client" starts looking like "DRM circumvention" — see §10. Shipping only the `raw` path keeps your project defensible and covers the large majority of the catalogue. [S]

### 5.4 Quality ceiling — read before choosing

The two paths may not reach the same quality, and the doc would be misleading without saying so:

- `download-info` is widely reported as **MP3-only (320 kbps ceiling)**. **[?]** — *asserted in several places but I did not confirm it against a live response. Verify before committing.*
- Every client that ships **FLAC does it through `get-file-info`**, and the one that documents its mechanics uses `encraw` + AES decryption to get there. [S] That is circumstantial but strong: if `download-info` served FLAC, they would not have built the harder path.

**Counterweight, and it matters:** a September 2026 probe of ~1,700 tracks found most are **MP3 320 anyway**, a few are 192-only, and **FLAC is simply not offered for many accounts** — the web UI shows no FLAC badge or setting either. [S] So the lossless ceiling may be far less of a real loss than it sounds.

### 5.5 Recommendation

**Use the mobile `download-info` path as primary**, and treat lossless as a stretch goal.

Rationale: you already hold an OAuth token from §3; there is no rotating secret; no scraper; no decryption; and for most of the catalogue on most accounts you end up at the same MP3 320 the web path would give you. The complexity of `get-file-info` buys you FLAC on a minority of tracks for a minority of accounts, plus a permanent maintenance tax.

Keep `get-file-info` documented as the secondary path — for lossless, and for the day `download-info` is retired.

**But settle open question #1 first** (§11): if `download-info` turns out to be degraded or gone, this recommendation inverts and M1 changes shape.

---

## 6. Моя волна / My Vibe

The nav on the live site labels it **"My Vibe"** in English, and the string `onyourwave` is still present in the web bundle [V] — the station identity hasn't changed under the rebrand.

### 6.1 What it actually is

Not a playlist. A **stateful server-side session**: you ask for a batch of tracks, you report what the user did with them, and the *next* batch is computed from those reports. There is no way to "fetch the wave" statically.

### 6.2 The loop

```
GET  /rotor/stations/dashboard              → what waves exist for this user
GET  /rotor/stations/list
GET  /rotor/station/{id}/info

GET  /rotor/station/{id}/tracks?settings2=True        (first fetch)
GET  /rotor/station/{id}/tracks?queue={lastTrackId}   (continuations)
     → { batchId, sequence: [ ~5–15 tracks ] }

POST /rotor/station/{id}/feedback?batch-id={batchId}
POST /rotor/station/{id}/settings3
```
[S]

Two corrections to an earlier draft, both **[V]** from the library source: the settings endpoint is **`settings3`**, not `settings2`; and `settings2`/`queue` are sent as **alternatives**, not together — the reference client overwrites the parameter map rather than merging it.

Station ids are `type:tag` pairs — `user:onyourwave` for the personal wave, plus `genre:*`, `mood:*`, `personal:*`. [S]

**The two listings are not interchangeable. [V] — verified live 2026-09-22.**

| | shape | size | contains `user:onyourwave` |
|---|---|---|---|
| `/rotor/stations/dashboard` | `{ dashboardId, pumpkin, stations: [entry] }` | 4, personalised | **yes** |
| `/rotor/stations/list` | a bare JSON **array** of entries | 699 | **no** |

So a wave picker has to merge both, dashboard first — the personal wave is only
reachable through the dashboard. Entry shape is the same in each:

```jsonc
{ "station": { "id": { "type": "genre", "tag": "pop" },   // join as "genre:pop"
               "name": "Поп",
               "icon": { "imageUrl": "avatars.yandex.net/…/%%",   // scheme-less,
                         "backgroundColor": "#FF6665" },          // like coverUri
               "fullImageUrl": "…/%%", "idForFrom": "genre-pop",
               "restrictions": {…}, "restrictions2": {…} },
  "settings": {…}, "settings2": {…}, "adParams": {…},
  "rupTitle": "…", "rupDescription": "…" }
```

`list` types and counts, as served: `micro-genre` 367, `genre` 161, `mood` 18,
`editorial` 19, `local-language` 14, `activity` 12, `epoch` 9, `mix-by-*` 91
across five sub-types, `personal` 4, `tempo` 3, `author` 1. [V]

Non-personal stations run the identical loop: `genre:jazz`, `mood:relaxed` and
`personal:never-heard` each returned a 5-track batch, accepted `radioStarted`,
and continued correctly with `?queue={lastTrackId}`. [V] Switching waves means
building a fresh session — the new station has its own `batchId`, so feedback
for the outgoing track must be sent **before** the switch or it lands on the
wrong station.

The `queue` parameter carries the **last played track id** so the server knows where to continue. Request the next batch when ~2 tracks remain, not when the queue empties — otherwise you get a gap between songs.

### 6.3 Settings

`settings2` accepts [S]:

| Field | Values |
|---|---|
| `moodEnergy` | `fun`, `active`, `calm`, `sad`, `all` |
| `diversity` | `favorite`, `popular`, `discover`, `default` |
| `language` | `russian`, `not-russian`, `any` |
| `type` | `rotor`, `generative` |

The station's `info` response carries `restrictions` enums with `possible_values` — **build your settings UI from that response** rather than hardcoding the lists, so new options appear for free. [S]

### 6.4 The feedback contract — not optional

```
radioStarted   → once, when the wave begins        (from: "your-app-name")
trackStarted   → when playback of each track begins
trackFinished  → with totalPlayedSeconds
skip           → with totalPlayedSeconds
like / dislike → user signals          [?] — see note
```
[S] for `radioStarted` / `trackStarted` / `trackFinished` / `skip`.

**[?]** `like`/`dislike` as *rotor feedback types* is unconfirmed — the documented library surface lists only the four above, and likes normally go through `/users/{uid}/likes/tracks/add-multiple`. Check whether the wave needs a separate rotor-scoped signal, or whether liking via the normal endpoint is enough for it to adapt.

Body carries `type`, `timestamp`, `from`, `trackId`, `totalPlayedSeconds`.

⚠️ **The body must be JSON. [V] — verified against the live API 2026-09-22.** A form-encoded body is rejected with `400 {"name":"condition is not met"}`, whatever the event type, `from` value, timestamp format, or client header. The reference Python library still posts form data here (`data=data`), so **it is stale on this endpoint** — a client copied from it will fail every feedback call while everything else keeps working, which is the worst possible failure mode: silent, and it degrades the wave.

```jsonc
// POST /rotor/station/user:onyourwave/feedback?batch-id=<batchId>
// Content-Type: application/json
{ "type": "trackStarted", "timestamp": 1790082872.65, "from": "yamusic", "trackId": "18697033" }
```

Ordering also matters: `radioStarted` is rejected unless a batch has already been fetched and its `batchId` is in hand. Fetch tracks first, then announce the radio. [V] The `batch-id` query parameter appeared optional in testing, but official clients send it. [V]

**Send all of them.** Clients that skip feedback report that the wave stops adapting and later batches degrade. [S] This is also the honest thing to do — the recommendation quality the user is paying for depends on it.

### 6.5 Newer wave surface

The bundle carries strings suggesting the wave has grown a richer model [V]: `multiwave`, `wave_source`, `wave_with_fixed_recommendations`, `wave_without_fixed_recommendations`, `wave_landing_screen`, `wave_agent_item`. There is also a dedicated `music-custom-wave-media.music.yandex.net` host for wave artwork/animations [V].

**[?]** Whether "custom wave" (the prompt-driven wave in the current app) runs on the same `/rotor/` endpoints or a new service is **not established**. Ship the classic `user:onyourwave` rotor first; investigate custom waves as a v2 feature.

---

## 7. Ynison — multi-device sync (optional, later)

Yandex's equivalent of Spotify Connect: a long-lived WebSocket protocol that synchronises playback state across the web player, phone apps and smart speakers. [S]

Shape [S]: a *redirector* handshake to find your state-service host, then a *state* WebSocket you keep alive, with exponential-backoff reconnects (5/10/30/60 s + jitter). The model is **passive-player** — Yandex owns the queue; your client reports track completion and waits to be told what's next.

Real value ("continue what I was listening to on my phone"), real cost. Put it behind a milestone, not in v1.

---

## 8. Recommended stack

```
┌─────────────────────────────────────────┐
│  UI — React/Svelte + TypeScript          │  webview
├─────────────────────────────────────────┤
│  Tauri IPC boundary                      │
├─────────────────────────────────────────┤
│  Rust core                               │
│   ├ auth      device flow + refresh      │
│   ├ store     token at rest (0600 file)  │
│   ├ api       typed client, retry/401    │
│   ├ rotor     wave session + feedback    │
│   └ player    rodio/symphonia + cache    │
└─────────────────────────────────────────┘
```

Design notes worth fixing early:

- **One HTTP layer, one place that refreshes tokens.** A 401 anywhere triggers refresh-and-retry once, then bubbles up as "please re-authenticate".
- **Model the rotor session as a state machine**, not as a list. `Idle → Started → Playing(batch, index) → NeedsBatch`. Feedback emission belongs to the state machine, not to UI event handlers — that's how you avoid losing events when the user closes the window mid-track.
- **Cache aggressively but not the audio URLs** — stream URLs are timestamp-signed and expire. Cache metadata and artwork; re-resolve the URL every play.
- **`X-Yandex-Music-Client`** on the mobile path: present yourself consistently. [S]

---

## 9. Roadmap

| Milestone | Contents |
|---|---|
| **M0 — spike** | Device-flow login → `/account/status` prints your name and Plus status. Proves the whole auth chain in a day. |
| **M1 — play one track** | `download-info` → resolve URL → decode → audio out. |
| **M2 — library** | Liked tracks, playlists, search, queue, basic transport controls. |
| **M3 — Моя волна** | Rotor session, batch prefetch at N-2, full feedback contract, settings UI built from `restrictions`. |
| **M4 — polish** | Offline cache, MPRIS / SMTC / MediaSession, keyboard shortcuts, lyrics. |
| **M5 — optional** | Ynison sync, custom waves, lossless via `get-file-info`. |

---

## 10. Risks — stated plainly

- **ToS.** Using an unofficial API and the official app's `client_id` is against Yandex's terms. Realistic exposure for a non-commercial open-source project is low but non-zero; it rises sharply if you monetise it or make it easy to bulk-download.
- **DRM.** Implementing `encraw`/`enclaw` decryption is materially different, legally, from consuming `raw` URLs. In the EU/US that touches anti-circumvention law. **Strong recommendation: don't ship decryption.** It also isn't needed for a listening client.
- **Key rotation** breaks the `get-file-info` path on Yandex's release schedule — already demonstrated between `v4.1603.1` and `v4.1626.1` [V]. Another reason to prefer `download-info`.
- **Account risk.** Aggressive request patterns can get an account flagged. Rate-limit, back off on 429, and behave like a player: no mass enumeration, no parallel bulk fetching.
- **Bus factor on upstream research.** Your project will effectively depend on `MarshalX/yandex-music-api` staying current. Vendor the parts you need rather than tracking a moving target blindly.
- **Be explicit in your README** that this is unofficial, requires the user's own Plus subscription, and is not affiliated with Yandex.

---

## 11. Open questions — verify before building

**Resolved — all verified end-to-end against a live Plus account on 2026-09-22 [V]:**

1. ~~Does `download-info` still work, and what is its quality ceiling?~~ **It works.** A resolved URL answers `HEAD 200`, `content-type: application/octet-stream`, `accept-ranges: bytes`, ~8.7 MB for a single track. Ranged requests are supported, so seeking works. The ceiling is MP3 — the URL template is `/get-mp3/`.
2. ~~Do `raw` transports cover the catalogue?~~ **Not applicable on this path.** `download-info` serves plain MP3 with no transport negotiation and no encryption, which is the main reason to prefer it.
3. ~~Does the device flow work without `client_secret`?~~ **No — it is required.** See §3A.
4. ~~The MD5 salt and URL assembly.~~ **Confirmed and working:** `sign = md5(SALT + path[1..] + s)`, `https://{host}/get-mp3/{sign}/{ts}{path}`.

**Still open:**

5. Are custom waves / `multiwave` served by `/rotor/`? **[?]** — affects later scope only.
6. Current rate limits. **[?]** — no public figures found.
7. Is `dislike` a valid rotor feedback type? **[?]** — still unverified; the player uses `skip` as the negative signal rather than guess.

---

## 12. Sources

Primary (read directly this session):
- `music.yandex.ru` live frontend bundle `v4.1626.1` — `get-file-info` resource layer, signature construction, `player.secretKey.web`, quality/transport enums, `onyourwave` and wave taxonomy strings.

Secondary:
- [MarshalX/yandex-music-api](https://github.com/MarshalX/yandex-music-api) — reference Python client. **Stale on rotor feedback:** it posts form-encoded bodies, which the live API rejected on 2026-09-22; JSON is required (§6.4). Treat its endpoint paths as reliable and its body encodings as needing a check.
- [Token acquisition docs](https://ym.marshal.dev/en/main/token.html) — device flow, implicit flow, `client_id`
- [MarshalX/yandex-music-token](https://github.com/MarshalX/yandex-music-token) — web/Android/extension token helpers
- [fgsfds1/yt-dlp-yandex-music-plugin](https://github.com/fgsfds1/yt-dlp-yandex-music-plugin) — `get-file-info` HMAC scheme, `strm.yandex.net`
- [trudenboy/ma-provider-yandex-music](https://github.com/trudenboy/ma-provider-yandex-music) — My Wave via rotor, FLAC, `encraw` AES
- [trudenboy/ma-provider-yandex-ynison](https://github.com/trudenboy/ma-provider-yandex-ynison) / [Music Assistant plugin docs](https://www.music-assistant.io/plugins/yandex-ynison/) — Ynison WebSocket behaviour
- [jrfrigat/YandexMusic](https://github.com/jrfrigat/YandexMusic) — .NET client, device-code + Ynison
- [Rotor settings reference](https://yandex-music.readthedocs.io/en/main/yandex_music.rotor.rotor_settings.html) — `moodEnergy` / `diversity` / `language` values
