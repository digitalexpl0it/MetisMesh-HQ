# Changelog

## Unreleased

## 0.1.21 — 2026-11-13

Package version `0.1.21` (Cargo / About). Windows Details may show `0.1.21.0`.

### Messages

- Smart live scroll: when near the bottom, new channel/DM traffic auto-scrolls to the newest message; when scrolled up, position is preserved and a “New message(s)” jump chip appears
- Open thread loads enough rows to cover unread (up to 200); live catch-up appends by id after the last loaded row so a connect drain cannot skip the middle and jump/drop half the pane
- Channel rail session contacts stay collapsed by default; expand the chevron (with count) to show who posted this session
- Message action Favorite uses amber highlight (filled when already pinned) like Reply/Quote/Block

### Radio

- Reconnect soft-error on connect queue drain seeds live `need_msg_drain` (same handoff as budget pause) so companion unread is not treated as empty (P3)
- Contact sync watermark is strictly per-radio: no live inherit from legacy global; one-shot migrate onto radios that already existed; remove pool-wide `list_nodes` seed-to-1; clear watermark when forgetting a radio (P3)

## 0.1.20 — 2026-11-12

Package version `0.1.20` (Cargo / About). Windows Details may show `0.1.20.0`.

### Radio

- Connect queue drain that pauses on budget/deadline seeds live `need_msg_drain` so remaining companion messages keep draining after Ready (O6)
- Clear UI when companion protocol is below v14: Device CLI soft notice + Overview chip; messaging/enrollment still OK (P1)
- Field log markers (S1): `sync ready`, existing MessagesWaiting/drain lines, plus `radio lock: … wait Nms` for operator/brief mutex stalls
- Status strip: Last RX age / Receiving… confidence; `last_rx_at` on RX insert (R3)

### UI

- Roster own 2.5s refresh; App shell badges/radio at 2s; Header + StatusStrip self-poll at 1s; Map stays on 5s overlay tick (R2)

### Setup / Linux

- Wizard BlueZ Install via apt, pacman, or dnf (not apt-only); distro-correct soft-dep hints (Q1–Q2)
- Settings → About: Linux / AppImage runtime note (host `webkit2gtk-4.1`, `dialout`/`uucp`, BlueZ) (Q3)

### Help

- **Live RX check** field matrix (USB/BLE/TCP × channel/DM × idle vs send-while-RX) plus quick soak steps; mirrored in `docs/RADIO_LIVE_RX.md`
- CLI note: local companion CLI needs protocol v14+; flash newer firmware from Settings → Flasher
- Connect companion: BlueZ / dialout-uucp / AppImage WebKit pointers

## 0.1.19 — 2026-11-12

Package version `0.1.19` (Cargo / About). Windows Details may show `0.1.19.0`.

### Radio

- Connect-time and path-fetch frames use the same listen-before-write helper as live `get_msg` (USB CDC race)
- Session tick: drain first; skip battery / contact retry while `need_msg_drain` so background brief-locks cannot starve RX
- Live drain after `MessagesWaiting` takes the operator lock budget; lock is released between 24-message batches (still no `start_auto_message_fetching`)
- Soft status “Receiving… radio busy, retrying.” while deferred drain / multi-tick receive is in progress

### Messages

- Instant list refresh when RX insert bumps `rx_seq` (1.5s poll kept as fallback)

## 0.1.18 — 2026-11-12

Package version `0.1.18` (Cargo / About). Windows Details may show `0.1.18.0`.

### Radio

- Live incoming messages: drain the companion queue on `MessagesWaiting` with lock-busy retry, plus a 2s opportunistic `get_msg` poll (listen-before-write). Fixes messages stuck until HQ restart ([issue #2](https://github.com/digitalexpl0it/MetisMesh-HQ/issues/2))
- Field-visible RX logs (`MessagesWaiting`, drain cleared/paused/deferred) for `--dev` / log file diagnosis

## 0.1.17 — 2026-11-11

Package version `0.1.17` (Cargo / About). Windows Details may show `0.1.17.0`.

### Setup / Linux

- USB serial wizard: use `uucp` when `dialout` is missing (Arch/CachyOS); stop re-offering Install after membership is written (log out still required for the live session)
- Flasher wizard Install: support **pacman** / **dnf** (esptool package; pipx then adafruit-nrfutil); hints no longer assume apt on Arch/Fedora
- Quiet Enchant “Error loading plugin” spam (missing aspell/hspell/…); skip WebKit spellcheck enable when no Hunspell `.dic` is installed

## 0.1.16 — 2026-11-11

Package version `0.1.16` (Cargo / About). Windows Details may show `0.1.16.0`.

### Packaging

- Linux **AppImage**: do **not** bundle `libwebkit2gtk` / `libjavascriptcoregtk` (Ubuntu WebKit ignores `WEBKIT_EXEC_PATH` and hardcodes Debian helper paths). Use host `webkit2gtk-4.1`; build verifies the image has no bundled WebKit ([issue #1](https://github.com/digitalexpl0it/MetisMesh-HQ/issues/1) follow-up on CachyOS)

### Flasher

- Clearer GitHub **rate-limit** errors on firmware sync (retry window + hint)
- Optional **GitHub personal access token** under Settings → Flasher (`github-firmware.token`, 0600) for higher API limits
- MeshCore release listing uses **ETag / 304** disk cache under `firmware/github-cache/` to reduce API calls

## 0.1.15 — 2026-11-10

Package version `0.1.15` (Cargo / About). Windows Details may show `0.1.15.0`.

### Packaging

- Linux **AppImage**: attempted WebKit helper bundling + `WEBKIT_EXEC_PATH` (insufficient on Arch — corrected in **0.1.16** by using host WebKit instead)

### Radio / CLI

- Device → Identity **Companion tools**: **Advert zero-hop** / **Advert flood** (labels aligned with MeshCore); Discover nearby stays under Network
- Device CLI → **RX log** tab: live overheard RF frames (SNR / RSSI / route / type); companion `LogData` subscribes only while that tab is open (unsubscribes when you leave unless map upload still needs it); in-memory ring, no payload dump

### Map

- Pin detail **Ping**: mesh binary status probe (SNR / RSSI / noise / battery toast); uses airtime; 8s rate limit shared with Trace
- Map Ping FX: smooth canvas beam from HQ pin to the target, then sonar on arrival; pin detail card shows Ping pending/ok/err; longer mesh timeouts + clearer failure copy
- Refresh contact / Ping: prefetch advert-path, push companion contact only for route/direct (never flood — Status cannot flood and path_len=-1 was wiping companion out_path); skip neighbours unless claimed; clearer “no path” copy; 0-hop channel RX marks direct route
- meshcore-rs: binary Status/Telemetry pending tags stay registered for the full operator wait (not only companion `suggested_timeout`) so late replies still match

### Messages

- Compose counter shows UTF-8 **bytes** (`n/max B`); channel compose starts at `0` with max reduced by the HQ name prefix (and reply framing), and clamps the draft so the wire still fits 160
- Phone-app contact shares `<pubkey:type:name>` in channel/DM bubbles become the same Add / Copy chip as `meshcore://contact/add`
- Channel bubble action bar (Reply / Quote / Copy / …): close the hover gap so moving from the bubble onto the pill no longer hides it (Windows WebView2)
- Compose spellcheck (Phase N): OS/WebView underlines + right-click suggestions on channel/DM/Broadcast compose; Settings → Preferences → **Spellcheck compose** (default on); native context menu allowed only on compose fields
- Compose bars use `<textarea>` (not `<input>`) so WebView spell underlines actually appear
- Linux: enable WebKitGTK WebContext spellcheck + dictionary languages (HTML `spellcheck` alone showed Cut/Copy/Paste with no suggestions)
- Compose typing: draft Signal reads stay in a compose dock (not the message list) so keystrokes no longer re-render the thread; spellcheck preference cached (no SQLite per key)
- Map Ping beam follows resolved hop pins when HQ has repeater GPS; otherwise straight HQ→target (dashed). Toast/detail note shows direct / hop count / “no repeater pins”
- **Advert path** on map pin, rail menus, roster, and message bubbles (companion path cache). **Ping** only when HQ already has a unicast route
- **Ping** also on Messages rail right-click (DM/Contacts), channel/DM bubble actions when a pubkey is known, and Roster row actions

### Help

- Map / Messages notes for Ping and compose spellcheck

### Local API

- Local API **v2**: OpenAPI at `/api/v1/openapi.json` and offline docs UI at `/api/v1/docs` (docs + OpenAPI public; data routes still need bearer); Settings → Local API → **Open API docs** / **Copy docs URL**
- Read channel/DM history (`/channels/{name}/messages`, `/dms`, `/dms/{pubkey}/messages`), infra detail + samples, expanded `/stats` and `/metrics`
- Heavy-route rate limit (messages/samples) in addition to the global per-minute cap; `429` includes `retry_after_secs`

### Flasher / Backup

- **Node identity kits** (`.metisnode`): USB-only encrypted backup of MeshCore `prv.key` / public key (+ name/radio snapshot), stored under `data/node-kits/` separate from HQ `.metisbak`
- Settings → Data → **Node identity kits**: backup/restore over USB, import/export/delete
- Desktop Flasher and LAN locker prompt to back up before flash and restore after; LAN API `GET/POST /api/node-kit(s)`

## 0.1.14 — 2026-11-08

Package version `0.1.14` (Cargo / About). Windows Details may show `0.1.14.0`.

### Flasher

- Opt-in **LAN flash locker** (Settings → Flasher): HTTPS with a self-signed cert, room PIN sessions, catalog + trusted firmware download, and browser Web Serial flash on remote laptops (Chrome/Edge). Default bind `0.0.0.0:443` with HTTP `:80` → HTTPS redirect; on Linux HQ can Polkit-prompt for low-port capability and restart (or use `8443`). Separate from the read-only Local API
- Locker board browse: maker/search filters, Web Serial vs download tabs, device → role slide-up with Flash / Download / Console / Setup
- **ESP32** Web Serial flash (SLIP) and **nRF52** Web Serial flash (Adafruit DFU `.zip`, vendored `jszip` + DFU) — SenseCAP / Wio Tracker L1 and similar are not download-only
- **OTAFIX**: Sync pulls every catalog bootloader name (`.uf2` and `.zip`) plus erase images (catalog erase + `.uf2` wipe sibling when present) for selected nRF devices; role sheet shows warning + UF2 download (drag-drop DFU — not Web Serial)
- LAN locker **Download ▴** menu (body-mounted, opens above the button): firmware, erase, and OTAFIX files when synced — escapes the sheet `transform`/`overflow` clip
- LAN locker **Enter DFU mode** on nRF roles (1200-baud bootloader reset for UF2 copy or serial DFU)
- LAN locker **Custom** firmware: flash a local `.bin` (ESP) or DFU `.zip` (nRF) from the laptop — not HQ-catalog verified
- **Repeater / Room Server setup** wizard on the locker: after flash, auto-runs serial CLI (`password`, `set name`, guest password, optional lat/lon/advert/repeat, `advert`, `reboot`)
- LAN locker **Help** sheet: browser/PIN, Web Serial (ESP vs nRF), OTAFIX UF2, Console/setup, empty-catalog notes

### Broadcast

- New **Broadcast** sidebar page: contact and channel groups, canned presets, queued one-by-one fan-out on the primary radio (intentional spacing; not an automatic bot reply)

### Welcome

- Setup wizard companion **name** step after connect (max one emoji when setting a name) plus a bottom icon step strip showing progress

### Map / Messages

- Position shares in message bodies render as map chips; related map host / bridge polish

### Docs

- README, offline runbook, Help, and todo updated for LAN locker / OTAFIX / Broadcast

## 0.1.13 — 2026-11-06

Package version `0.1.13` (Cargo / About). Windows Details may show `0.1.13.0`.

### Map

- Fix broken contact QR on map pin popups: QR `<img>` URLs now use the OS-aware `metis` asset helper (`http://metis.localhost/…` on Windows). Plain `metis://` never loads in WebView2 image tags

## 0.1.12 — 2026-11-06

Package version `0.1.12` (Cargo / About). Windows Details may show `0.1.12.0`.

### Platforms / packaging

- Release artifacts: **Windows** `.zip`, **Ubuntu 26.04** `.deb`, **Linux** `.tar.gz`, **Linux** `.AppImage`
- New `packaging/linux/build-deb.sh` — installs under `/usr/lib/metis-mesh-hq` with a `/usr/bin/metis-mesh-hq` launcher, desktop entry, and Depends on Ubuntu 26.04 WebKit/GTK packages
- CI job renamed to **Linux** / **Windows**; Linux job builds deb + tarball + AppImage on `ubuntu-26.04`

## 0.1.11 — 2026-11-06

Package version `0.1.11` (Cargo / About). Windows Details may show `0.1.11.0`.

### Map

- Fix “MetisMap bridge missing” on Windows release builds: MapLibre / PMTiles / map_bridge are served from the embedded `metis` protocol (same as outline GeoJSON). GitHub `.exe` zips never shipped an `assets/` folder, so dioxus `asset!()` script URLs 404’d and the map never started. Init also waits up to 20s for the bridge instead of failing after 180ms
- Release packages now include the `assets/` folder next to the binary as a backup for any remaining `asset!()` URLs

### Platforms / packaging

- Supported releases are **Windows** and **Linux** only — macOS CI and release artifacts are dropped
- Linux AppImage (`metis-mesh-hq-x86_64.AppImage`) built with linuxdeploy + GTK shell plugin so GTK/WebKit ship inside the image; use this on non-Ubuntu or older distros. Host still needs BlueZ (Bluetooth) and dialout/udev for USB serial. (WebKit helper processes / Arch fix landed in **0.1.15**.)
- Ubuntu 26.04 `.tar.gz` remains for thin installs that already have the system WebKit/GTK packages

## 0.1.10 — 2026-11-06

Package version `0.1.10` (Cargo / About). Windows Details may show `0.1.10.0`.

### HTTPS / downloads

- Fix “Unknown Scheme / cannot make HTTPS request because no TLS backend is configured” on map place lookup, firmware Sync, presets, and other `ureq::get` calls. 0.1.9 enabled only ureq’s `native-tls` feature, which does not attach a backend to the crate-level helpers; 0.1.10 uses `tls` + `native-certs` (rustls with the OS trust store, including Zscaler roots Windows already trusts)
- Firmware Sync toasts on success or failure; last sync error stays visible under the library banner (it used to be hidden whenever readiness was shown)

## 0.1.9 — 2026-11-06

Package version `0.1.9` (Cargo / About). Windows Details may show `0.1.9.0`.

### Log file

- HQ writes its log to `logs/` in the data folder (Windows `%APPDATA%\MetisMeshHQ\logs`), so you can read it outside the app. Each launch starts a new `metis-hq.log` and older ones move to `metis-hq.1.log` through `metis-hq.4.log`. At most 5 files of 5 MB each are kept (25 MB at most), and a file that fills up mid-session rotates too
- Settings → Diagnostics → Log file: a "Save log to disk" switch (on by default; turning it off stops writing at once), the current file path with Copy, file count and size on disk, Open log folder, and Delete older logs
- Info, warning, and error lines from `meshcore-rs` and `btleplug` now show in the App log and the file. Debug output from these libraries is never captured because it contains raw radio frames with channel secrets. App lines are redacted the same way as the in-app log
- Panics are written to the log file before the app exits, so a crash leaves a record

### Windows install / profile

- WebView2 cache uses a stable `%LOCALAPPDATA%\MetisMeshHQ\webview` path (dioxus default was `%LOCALAPPDATA%\<exe-name>`, so each `metis-mesh-hq-0.1.x.exe` created a new folder). Old versioned folders under Local are safe to delete. HQ data stays in `%APPDATA%\MetisMeshHQ`

### Offline map

- Outline basemap + PMTiles use `http://metis.localhost/…` on Windows (WebView2 will not fetch `metis://` for MapLibre sources) so land outlines show and Place on map works without a ZIP first
- Download card keeps status after Working… ends; ZIP/place downloads toast on success or error instead of going silent

### Windows BLE

- After pair/connect: welcome step shows pairing → connecting → syncing feedback with a throbber; Continue/Skip stay disabled until the attempt finishes
- Welcome Bluetooth list: Selected / Pairing… / Connecting… / Connected badge + checkmark on the linked row; Pair button becomes Connected with a “Companion linked” hint
- Stop reconnect loops when channel sync times out after a long contact dump — stay linked, settle, retry channel setup in the background
- BLE settle delays after GATT open and after contacts (longer on Windows); one get_channel retry before deferring
- APPSTART / device query listen before write (meshcore-rs race dropped fast SelfInfo → “Reading companion identity…” timeout); longer Windows settle
- Windows BLE: stop auto-reconnect after APPSTART / device-not-found (reconnect loop was resetting companions and hiding them from Scan); drop GATT so the radio can advertise again; single APPSTART attempt + longer post-GATT settle
- Vendored `meshcore-rs` BLE: write-with-response (MeshCore-recommended; WithoutResponse often silent-fails on Win11), arm notify stream before any write, subscribe retry, 8s device find + known-peripheral match
- **Windows Bluetooth pairing (root cause of “never answered APPSTART”):** MeshCore firmware (ESP32 and nRF52) ignores every frame until the link is encrypted with a PIN bond, and HQ never paired on Windows. Pair with PIN now runs WinRT custom pairing with the operator’s PIN (EncryptionAndAuthentication); an existing bond without PIN protection is removed and re-made; pairing results map to clear messages (wrong PIN, radio held by phone, timeout)
- Vendored `meshcore-rs` BLE: a refused write/subscribe on an unbonded link (`GattCommunicationStatus` ProtocolError/AccessDenied) now fails at once with “Bluetooth link is not paired…” instead of falling back to write-without-response (silently dropped by the firmware) and timing out; auto-reconnect stops on it
- APPSTART bytes match meshcore_py (firmware ignores bytes 1–7); wait for ATT MTU after connect; log SelfInfo parse failures

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
