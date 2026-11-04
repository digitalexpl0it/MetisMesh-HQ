# Changelog

## Unreleased

### Overview · Repeaters

- Fleet dashboard: summary tiles, compact claim cards, **Details for** dropdown with KPIs + charts
- L1: detail chart window chips **15m / 1h / 6h / 24h / 7d** (persisted `overview_repeater_chart_window`)
- L2: Battery and Queue KPI modes **Chart / Gauge / Numeric** (persisted); new SVG `Gauge` widget
- KPI sparklines use probe timestamps on hover (not `#N` sample index); Overview companion stats likewise use hour labels + units
- Duplicate full Battery chart under repeater KPIs removed (tile Chart mode is the battery trend)
- TX power shows a hint when missing from binary status (Infra radio CLI / prior value)

### Map · paths

- Operator **Trace** for tracked or claimed pins (`CMD_GET_ADVERT_PATH` 0x2A); local only — no channel/DM
- Path fetch listens for advert-path / PathUpdate, fails fast on cache miss, falls back to contact `out_path` / direct / last stored path
- Passive **Traffic** edge overlay from overheard message paths (All GPS vs Tracked/Claimed)
- Map Filter / Settings menus; Selected label next to Trace

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
