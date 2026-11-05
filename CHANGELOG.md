# Changelog

## Unreleased

### Windows polish

- Default EmergHQ brand logo uses an embedded data URL (WebView2 cannot load `metis://` in `<img>`); custom logos use `http://metis.localhost/…`
- Bluetooth PIN slide-down sits above the welcome overlay so first-run Pair is usable
- Welcome soft-deps probes are cached and Windows helper processes use `CREATE_NO_WINDOW` (stops UI stalls / flashing consoles during setup)
- Release builds use `windows_subsystem = "windows"` so a cmd host does not stay open
- Taskbar identity: set AppUserModelID + window icon from the mesh-head mark
- BLE reconnect: skip aggressive GATT force-disconnect on session loss (avoids connect/disconnect loops); longer settle backoff
- Offline map download: fetch Windows/Darwin `go-pmtiles` `.zip` (was looking for `.tar.gz`), extract with the zip crate, use `where` instead of `sh`
- Map +/- zoom buttons owned by Dioxus (JS-injected controls were wiped on re-render)
- Map trackpad zoom: HQ owns wheel/pinch (ctrl+wheel) so WebKitGTK/WebView2 cannot steal page zoom; two-finger scroll and pinch zoom the map on Linux and Windows
- Messages page uses flex-fill main pane (WebKitGTK `height:100%` left a dead gap under the thread); reset WebView zoom to 1.0 on launch
- Enrollment DMs: reply only to `#signup` / `#status` (same `#` command marker as channel bots); bare words and casual chat stay silent
- BLE: three consecutive battery/channel timeouts while “connected” end the session and reconnect (was stuck Connected with `BLE TX error: Not connected`)
- Messages compose **+** menu (channel and DM): my contact, my position, share channel name, share contact link, share map location; **Quick message (DM)** on channels only — channel share never includes secrets
- Channel cards truncate long previews (URLs no longer bleed into the next card)
- Chat bubbles: timestamp above; larger avatar/name inside the bubble with real padding; hops + sent/received below
- Join channel: private (name + hex secret), restore Public, hash/hashtag channel; create channel optional advanced region tag
- Messages list scrolls again under `hq-main-fill`; Create/join is one button → slide-down option picker → form
- Companion already held (phone GATT / busy serial / early connect failures): stop auto-reconnect after a short streak and show “Companion busy — disconnect… then Connect again” instead of looping Reconnecting
- Composer emoji picker: many more glyphs (reactions, faces, hands, mesh, status, weather, nature, food, people); panel scrolls
- Messages: switching channels/DMs while a thread is open no longer shows a blank chat (paint from freshly loaded messages; scroller remounts per thread)
- Infra Radio/TX settings: Get TX fills the dBm field; Get radio updates the Radio line; Get/Set TX and Get radio show a throbber while waiting on the mesh reply
- Infra admin gets: capture replies into Status metrics / Settings fields (intervals, lat/lon, clock, repeat, …) with a green result banner and button throbbers — not only the bottom CLI transcript
- Popovers (Messages compose + / emoji, Map Settings / Filters): click outside the menu to dismiss
- Messages rail: **Channels / DMs / Contacts** tabs with icons; Contacts **Roster** auto-fills from the node list; custom sections (with optional emoji icon) on Channels and Contacts; star/favorite pins contacts; **Add** on `meshcore://contact/add` chips and DM bubbles; click channel or contact opens the chat thread; rail section tables created on every DB open
- Messages rail item menus: move-to-section lives under **settings** (gear) and **right-click**; Move submenu lists sections plus **Add section…** (create then auto-move)
- Channel bubble **Favorite** favorites the sender contact (not the channel); Messages left/right panes are drag-resizable (width remembered)
- Section emoji uses the compose emoji picker button; Default keep is a single menu button instead of six chips
- Messages rail: clearer col-resize cursor on the pane splitter; per-tab fuzzy search filters Channels / DMs / Contacts
- Messages bubble **Favorite** resolves sender keys more reliably and refreshes the rail (Contacts → Pinned / DMs Favorites)
- Pane splitter uses the native desktop col-resize cursor and keeps drag capture on a always-mounted overlay
- DMs tab has its own **Section** button and custom sections (no longer lists Channels sections like “Locals”); DMs builtins seeded under the `dms` tab
- Channel-only senders (no pubkey yet) can still be Favorited as a Contacts → Pinned name bookmark; upgrades when an advert arrives
- DMs search is tab-scoped again (DMs stuck in old Channels sections no longer disappear); search also matches roster contacts to start a chat
- Emoji picker: type common words (coffee, signal, heart…) to filter — same search on every emoji button
- Messages pane splitter: force col-resize via WebView CSS class + native EwResize (WebKit was keeping the arrow)
- DMs rail search filters DM threads only (no roster dump); short queries match names, not every pubkey hex digit
- Channels: **Default keep** is a top-right button beside Create / join; options open in a slide-down panel
- Default keep slide-down lists durations inline (dropdown was clipped under the panel footer)
- Create / join channel picker: icons + soft color wells per action
- Rail context/settings menus host on a full-viewport shell (WebKit was clipping under the rail); Move to section flies out to the right again (flips up/left near edges)
- Name-only favorites show “awaiting advert” and auto-upgrade when the roster/advert resolves that name
- Splitter cursor: await WebView style inject + native `ColResize` (fire-and-forget eval never applied)
- Message bubble actions: drop redundant Pin/Contacts (Favorite already saves to Contacts → Pinned)
- Awaiting-advert pin feedback is a toast (no longer a sticky rail banner); contact/DM right-click adds **Fetch advert** (companion path fetch)
- Favorite feedback is toast-only (no sticky line under the composer); contact/DM hover tips use short name + 8-char prefix (not the full pubkey)

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
