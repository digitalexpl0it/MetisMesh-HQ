# Changelog

## Unreleased

### Offline map

- Outline basemap + PMTiles use `http://metis.localhost/…` on Windows (WebView2 will not fetch `metis://` for MapLibre sources) so land outlines show and Place on map works without a ZIP first
- Download card keeps status after Working… ends; ZIP/place downloads toast on success or error instead of going silent
- HTTP downloads (map packs, firmware sync, presets) use OS native TLS so Zscaler / corporate roots in the Windows trust store work — no cert-ignore option

### Windows BLE

- After pair/connect: welcome step shows pairing → connecting → syncing feedback with a throbber; Continue/Skip stay disabled until the attempt finishes
- Stop reconnect loops when channel sync times out after a long contact dump — stay linked, settle, retry channel setup in the background
- BLE settle delays after GATT open and after contacts (longer on Windows); one get_channel retry before deferring

## 0.1.6 — 2026-11-06

Package version `0.1.6` (Cargo / About). Windows Details may show `0.1.6.0`.

Messages rail overhaul, MeshCore-aligned DMs, Windows desktop polish, and Infra admin capture — everything since 0.1.4.

### Messages · rail

- **Channels / DMs / Contacts** tabs with icons; Contacts **Roster** auto-fills from the node list
- Custom sections (optional emoji) on Channels, DMs, and Contacts; star/favorite pins; rail section tables created on every DB open
- Item menus under **settings** (gear) and **right-click**: Move to section, **Add section…**, Details, Block/Unblock, Refresh contact
- Session speakers indented under each channel (cleared on app launch; filled when someone posts live)
- DM only when a real public key is already known (contact card / companion contact) — channel display names alone never open a DM
- Channel-only senders can still be Favorited as a Contacts → Pinned name bookmark; upgrades when an advert arrives
- Per-tab fuzzy search; DMs search is thread-scoped (no roster dump)
- Left/right panes drag-resizable (width remembered); col-resize cursor forced through WebView CSS + native cursor
- Zebra row strips; blocked contacts stay listed but greyed out
- **Refresh contact** throbber until path/status/telemetry finish; Details slide-down for channel or contact/DM
- Slide-down panels sit above the splitter; Default keep is a top-right button → duration slide-down

### Messages · chat

- Compose **+** menu: my contact, my position, share channel (`meshcore://channel/add` for Public/hash; private name-only), share contact, share map location — private secrets never go in chat; Quick DM removed
- `meshcore://contact/add` chips: **Add** / **Copy**; `meshcore://channel/add` chips: **Join** / **Copy**
- Channel bubble actions: Reply / Quote / Copy / Favorite / Block (no bubble DM — MeshCore opens DMs from contacts with a key)
- Chat bubbles: timestamp above; larger avatar/name; hops + sent/received below; action pill below the bubble
- New-messages divider + unread badge until scrolled past (top of pane or bottom-of-thread); works when last-read is older than the loaded window
- Join channel: private (name + hex secret), restore Public, hash/hashtag; create channel optional region tag
- Create / join is one button → slide-down picker → form; channel cards truncate long previews
- Composer emoji picker: larger glyph set + word filter; popovers dismiss on outside click
- Switching channels/DMs no longer blanks the thread; list scrolls under `hq-main-fill`
- Contact sync backfills `sender_pubkey` on recent name-only channel rows when the roster uniquely matches

### Windows / desktop

- Default EmergHQ brand logo uses an embedded data URL (WebView2 cannot load `metis://` in `<img>`); custom logos use `http://metis.localhost/…`
- Bluetooth PIN slide-down sits above the welcome overlay; welcome soft-deps probes cached; Windows helpers use `CREATE_NO_WINDOW`
- Release builds use `windows_subsystem = "windows"`; taskbar AppUserModelID + mesh-head icon
- BLE reconnect: skip aggressive GATT force-disconnect on session loss; three consecutive battery/channel timeouts end the session and reconnect
- Companion already held (phone GATT / busy serial): stop auto-reconnect and show “Companion busy…” instead of looping
- Offline map download: Windows/Darwin `go-pmtiles` `.zip` + zip crate; MapLibre +/- and trackpad zoom owned by HQ (no page zoom steal)
- Enrollment DMs: reply only to `#signup` / `#status`; bare words stay silent

### Infra / Roster

- Infra Radio/TX: Get TX fills dBm; Get radio updates the Radio line; Get/Set TX and Get radio show throbbers
- Infra admin gets: capture replies into Status metrics / Settings fields with green result banners and button throbbers
- Roster Actions: icon-only colored buttons with tooltips (Details / Claim / Message / channel / Revoke)

## 0.1.5 — 2026-11-05

Intermediate package bump (`0.1.5` on tag `v0.1.6` history). Release notes for that window are folded into **0.1.6** above.

## 0.1.4.0 — 2026-11-04

Package version `0.1.4` (Cargo / About). Windows Details may show `0.1.4.0`.

### Desktop packaging

- Application icon: mesh-head mark cropped from the EmergHQ / Metis brand logo (`assets/app_icon.png` / `.ico`); set on the desktop window
- Linux: GTK pixbuf icon + user `.desktop` / hicolor install so Wayland docks show the mark (tao `with_icon` alone is not enough); install runs before the window maps so first launch is not a blank placeholder
- Windows `.exe` Properties → Details: File description, Product name/version, Copyright via `winres` (`build.rs`)

### Local JSON API + Home Assistant

- Opt-in Settings → Services → **Local API**: bind (default `127.0.0.1`), port `8787`, prefix `/api/v1`
- Bearer token in `data/local-api.token` (`0600`); generate / rotate / copy / clear; required on every request
- Read-only JSON: `/health`, `/status`, `/nodes`, `/nodes/{id}`, `/infra`, `/channels`, `/stats` (no secrets, no message bodies)
- Explicit deny paths return `403`; Settings/Help list what is never exposed
- Per-IP rate limit (default 120/min, failed-auth tighter); `Retry-After` on `429`; audit only on real setting changes; `/health` includes `api_version: 1`
- Help → Local API: curl examples and Home Assistant REST sensor YAML
- Opt-in **Home Assistant MQTT discovery** (separate from Weather MQTT): retained config + state every 10–300s for companion online/state, enrolled count, first claimed repeater battery/noise/probed_at; Settings Fill from Weather MQTT; Help Mosquitto + HA discovery note

### Overview · Repeaters

- Fleet dashboard: summary tiles, compact claim cards, **Details for** dropdown with KPIs + charts
- Detail chart window chips **15m / 1h / 6h / 24h / 7d** (persisted `overview_repeater_chart_window`)
- Battery and Queue KPI modes **Chart / Gauge / Numeric** (persisted); new SVG `Gauge` widget
- Star detail KPI tiles (Battery/Noise/RSSI/SNR/Queue); starred order persists as `overview_repeater_kpi_favorites`
- Display-only hard gates + Hampel MAD filter on spark/RF/rate series; note when outliers are hidden; latest KPI values stay raw
- KPI sparklines use probe timestamps on hover (not `#N` sample index); Overview companion stats likewise use hour labels + units
- Packet-rate and airtime charts: interval-end date/time on hover and axis; rate series stay time-aligned; skip airtime counter-jump spikes
- Duplicate full Battery chart under repeater KPIs removed (tile Chart mode is the battery trend)
- TX power shows a hint when missing from binary status (Infra radio CLI / prior value)

### Map · paths

- Operator **Trace** for tracked or claimed pins (`CMD_GET_ADVERT_PATH` 0x2A); local only — no channel/DM
- Path fetch listens for advert-path / PathUpdate, fails fast on cache miss, falls back to contact `out_path` / direct / last stored path
- Passive **Traffic** edge overlay from overheard message paths (All GPS vs Tracked/Claimed)
- Map Filter / Settings menus; Selected label next to Trace
- Legend badge counts for HQ / Chat / Repeater / Room / Sensor pins and solid / dashed path lines (respects current filters)

### Messages · badges

- Channel unread: inbound `rx` **and** HQ `origin=bot` TX (services / bot replies) count toward rail badges
- Operator Compose TX (`origin=local`) still does not badge
- Last-read cursors seeded when channels are created so never-opened channels (e.g. METIS-INFO) can badge without a prior open

### Infra · TX CLI

- Get TX / remote CLI ack handling ignores empty/noise lines so capture does not clear on send-ack alone
- Map upload posts to map.meshcore.**io** and follows redirects (fixes 307 from .dev)

## 0.1.3 — 2026-11-03

### Ship

- Standalone Linux / Windows / macOS release binaries via CI (`cargo build --release`); tag `v*` publishes a GitHub Release
- CI fixes: `libudev-dev` on Linux, Windows-safe USB by-id test, unique SQLite snapshot temp paths
- Backup restore tests use a file-backed DB; WAL pragma allows in-memory only for unit-test connections
- Data folder moved to the user profile: Linux `~/.MetisMeshHQ`, macOS `…/MetisMeshHQ`, Windows `%APPDATA%\MetisMeshHQ` (`METIS_DATA_DIR` still overrides)
- SQLite WAL durability (H3): open/restore assert `journal_mode=wal`, `busy_timeout=5s`, `synchronous=NORMAL`; docs note `metis.db-wal` / `metis.db-shm` beside the main file
- Default EmergHQ header logo embedded in the binary (release packages no longer show a broken image)
- First-run welcome wizard: install runtime deps (pkexec apt / WebView2) and optional companion connect; Settings → About (version, author, MIT, GitHub, data folder)
- Welcome System setup Install buttons for optional flashers (`esptool` via apt, `adafruit-nrfutil` via pipx; Windows via PowerShell + winget/pip)
- Windows WebView2 bootstrapper install uses PowerShell (`Invoke-WebRequest`) instead of the Rust HTTP client
- Scan / Refresh USB buttons show a throbber while working (welcome + Settings)
- Linux baseline is Ubuntu 26.04+ (CI job/artifact `ubuntu-26.04`); critical probe uses `libgtk-3-0t64`
- `.cursor/` ignored for git (local plans/rules stay on disk)

### Radio pool

- USB serial companions on the same pool as BLE
- TCP companions (Wi-Fi / Ethernet firmware, default port **5000**); Settings Network card
- Multi-radio pool with primary-radio pick; bot / enrollment TX from primary companion
- Contact hears tagged by `radio_id` (F4): `node_radios` + `last_radio_id`; roster/map stay one row per pubkey; per-radio `contacts_since`
- USB sticky identity (F5b): store VID/PID/serial; rematch open path across tty/COM renumeration without changing `radio_id`

### Infrastructure

- Claimed repeaters / rooms / sensors (`claimed_infra`) with Infrastructure page
- Verify-on-claim via `CMD_SEND_LOGIN` + admin ACL proof; passwords never stored
- Status probe + remote CLI through the primary companion; Overview strip only when claims exist

### Tracker, map, services

- Tracker whitelist + map trails / stale styling
- Message sonar / warp-in map presence
- Weather, NWS/Amber alerts, earthquakes on METIS-INFO; optional METIS-NET peer channel
- Local MQTT weather station (D1c): bare `#weather` prefers a fresh Mosquitto reading (e.g. Ecowitt GW3002 via HA/WeeWX); ZIP/sched stay Open-Meteo
- Local Ecowitt ingest + Weather page (D1d): custom-server push and gateway `get_livedata_info` pull (GW1200 / WittBoy / GW300x); shared station cache; no cloud Ambient
- Optional map.meshcore.dev uploader (E4): default off; LogData infra GPS ADVERTs only; operator Authorize on primary companion (no private key in DB/backup)
- Encrypted backup/restore; contact bulk import/export
- Offline firmware locker + selective Manage overlay + USB flasher
- Firmware sync verifies SHA-256 against GitHub release `digest` when present, stores hashes in `index.json`, re-checks on-disk files, and re-downloads corrupt/incomplete assets (up to 3 attempts)
- Outage readiness: Flasher ready banner, flash blocked on integrity failure, Check integrity / Export pack / Import pack (`.zip`); full offline runbook in Help (About keeps a short checklist + Open Help); `docs/OFFLINE_RUNBOOK.md` mirrors Help for GitHub
- Channel/DM unread: received `rx` plus HQ bot/service TX (`origin=bot`); operator Compose TX does not inflate badges; last-read cursors seeded for new channels
- Firmware Manage Save & Close pinned in a sticky slide-down footer so they stay visible on short windows

### Messages

- Sliding newest-N message window with older-page load
- HQ `@mention` badges on Messages / Overview
- Channel Block by display name when no roster pubkey (MeshCore mobile parity); promote to pubkey when heard
- Messages appearance: global + per-channel solid/gradient/opacity/text (Settings → Messages appearance; channel Look / rail gear)
- Sender avatars and soft bubble tints; Keep control stays on one line with Look

### UX

- In-app Help page (nav above Settings): operator guide + shareable radio-user signup/channel steps; Settings → About → Open Help
- Nav order by use (Overview → Messages → Inbox → Roster → Map → Infrastructure → Tracker → CLI; Help then Settings)
- Expanded-nav fuzzy jump (pages + tab/keyword index)
- Zebra rows on Roster / Infrastructure / Tracker (`.row-zebra` in `form.css`)
- GPS locate icon green within a week of last heard, red when older
- Shell layout: opaque top bar + flex overflow so pages do not paint under the banner
- Overview chips: system row (Alerts / Radio / Backup / Firmware) + messages row (Unread / Mentions / Channels / Direct); tone colors; sonar when Unread / Mentions / Direct are live; whole-card navigation
- OverviewStat cards tinted and clickable (Roster / Inbox / Map / Messages)
- README capability list + Overview screenshot (`Screenshots/overview.png`)

## 0.1.2 — 2026-11-02

- Map fixes and polish

## 0.1.1 — 2026-11-02

- Patch release (same tree as 0.1.0 tagging); see `v0.1.1`

## 0.1.0 — 2026-10-31

First cut you can actually run as a headquarters.

### Radio and enrollment

- BLE companion connect with the six-digit PIN (PIN is not echoed in the message list)
- Contact sync, clock set, queued traffic drain, auto-fetch
- Enrollment flow: pending → approve / deny, channel secrets only after approval
- Bot commands for enrolled nodes; heard-but-not-enrolled nodes stay in the DB
- CLI page that talks to the radio without racing the subscribe path

### Map

- Offline-first MapLibre (bundled JS/CSS/style — no CDN)
- Node pins with type, hops, last heard, RF when we have it
- Optional PMTiles packs for real offline basemaps
- Favorites and camera position stick around between sessions

### Contact QR

- Background worker encodes `meshcore://contact/add` QRs into `data/qr/`
- SQLite keeps `qr_file` / `qr_uri` so the map never encodes on refresh
- Identity page can show / print the HQ contact QR
- Pin details pull the cached SVG when it is ready

### Messages and channels

- Private channels with random 16-byte secrets (keyring, or a `0600` fallback file)
- Per-channel message keep, plus a default keep in Settings
- Overview traffic graphs and the usual inbox / messages views

### Housekeeping

- SQLite in WAL under `data/`
- Audit log for operator sends
- Settings for HQ name, radio presets, purge, and keyring status

Still ahead: HQ-to-HQ linking, standalone signed installers, packaging polish.
