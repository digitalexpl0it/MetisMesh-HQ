# Metis Mesh HQ — map presence, Tracker, services

Game plan from potato-mesh (sonar UX), meshcore-bot (weather/quake), and Tracker whitelist.
Build in order; check boxes as we finish.

## Principles

- Local HQ owns the radio(s); one Tokio task per companion; phone app must drop BLE when HQ holds that link.
- Mesh text stays plain; alerts labeled with source/time (not FEMA/NWS cosplay).
- GPS tracks stay local; Tracker is an operator whitelist (share-location is on *their* radio).
- Don’t invent positions; ignore `0,0`; site pin ≠ live GPS.
- Ephemeral map FX only — keep GL pins / pan perf intact.

## Phase A — Map presence

- [x] **A1** Message sonar: 2 thin rings (~1–1.2s, ~2× icon) on TX/RX when node has map coords
- [x] **A2** Warp-in + sonar when a device first appears on the map with coords
- [x] **A3** Short sonar when a *tracked* node’s lat/lon changes (after Tracker)

Refs: [potato-mesh](https://github.com/l5yth/potato-mesh) LV5 wave (`emitMarkerWave` / `.live-flash-wave`)

## Phase B — Tracker

- [x] **B1** Whitelist store: pubkey, label, added_at, notes (`tracked_nodes` or settings)
- [x] **B2** Tracker page: add/remove from roster; note “moves only if that radio shares location”
- [x] **B3** Map Tracker mode: tracked + HQ filter; trails from `positions` (1h / 6h / day / all)
- [x] **B4** Stale styling / last-seen when advert or position age > N minutes
- [x] **B5** (optional) Soft follow / recenter on selected tracked node when a new point lands

Do **not** poll the whole mesh for GPS. Rely on adverts / contact sync. Any later probe: whitelist only + rate-limited.

## Phase C — Capture parity (MeshCore-shaped)

- [x] **C** Audit (2026-11-01): MeshCore companion capture is in good shape for HQ
  - **OK:** contacts + `raw_json` snapshot; lat/lon (ignore 0,0) + `positions` on real move; message hop/SNR/`raw_json`; DM signature; DM path via `CMD_GET_ADVERT_PATH` (0x2A) → out_path → path_update; RF samples (noise/RSSI/SNR); HQ battery `telemetry_samples`
  - **Fixed:** channel RX now binds `sender_pubkey` when the channel name matches exactly one roster node, touches `last_heard`, and runs the same path resolve as DMs (no guessing on name collisions)
  - **Skip:** potato Meshtastic env telemetry warehouse (temp/PM/wind/etc.) — not on MeshCore companion frames
  - **Later niceties (optional):** message-list flash, activity sparkline, neighbor overlay, operator traceroute UI; auto status-probe for tracked nodes only (rate-limited)


## Phase D — Info services

Cookbook: [agessaman/meshcore-bot](https://github.com/agessaman/meshcore-bot/) —
[Weather](https://agessaman.github.io/meshcore-bot/weather-service/) ·
[Earthquake](https://agessaman.github.io/meshcore-bot/earthquake-service/)

- [x] **D1** Weather: Open-Meteo on **METIS-INFO** (`#weather [zip]`); clean `WX City ZIP:` lines; humidity; ≤160 bytes; `#weather+` multi-frame detail (no source stamp on air)
- [x] **D1b** Scheduled ZIP weather: Settings ZIP list + interval (default 60m); one ZIP/min when due; silent otherwise
- [x] **D1c** Local MQTT weather station: subscribe to custom topics (meshcore-bot–style `MqttWeather` + Weather mapping); prefer station readings for HQ/`#weather` when fresh; fall back to Open-Meteo when MQTT is off/stale/missing fields; Settings for broker, topics, unit labels; same ≤160 mesh line shape; Ecowitt GW3002 via HA/WeeWX→Mosquitto (no native Ecowitt HTTP)
- [x] **D1d** Local Ecowitt stations + Weather page: custom-server push + `get_livedata_info` pull (GW1200 / WittBoy / GW300x); unified station cache with MQTT; operator Weather nav page; no cloud Ambient
- [x] **D2** NWS weather alerts: poll near HQ site pin; Inbox confirm unless auto-relay → METIS-INFO
- [x] **D2b** Amber (CAE) + civil emergencies (CAP codes): `#amber` / `#alert`; `#amber+` / `#alert+` multi-frame CAP description; poll + auto toggles; state setting
- [x] **D3** Earthquakes: USGS bbox + magnitude + dedupe; post **only when new**; `#quake` on demand
- [x] HQ-owned `services` module + poller (not a second companion). DMs stay signup/status only.
- [x] **D4** Separate private peer channel (e.g. METIS-NET) for human emergency traffic (METIS-INFO = bot/info); both names settings-renameable; approve sends both secrets

## Phase E — Backup, contacts, optional public map

HQ durability and sharing. One-off contact QR already exists (`meshcore://contact/add`); this phase is bulk + disaster recovery.

- [x] **E1** Backup / restore: encrypted archive (passphrase) of SQLite + channel secrets from keyring/`data/`; restore with confirm + audit entry; never leave secrets in cleartext zip
- [x] **E2** Contact bulk export: JSON (or MeshCore-compatible list) of pubkey, name, type, optional last lat/lon — no secrets / private keys
- [x] **E3** Contact bulk import into HQ DB: merge by pubkey; do **not** auto-push onto the radio contact table; “add to radio” stays deliberate
- [x] **E4** (optional) Map uploader → [map.meshcore.dev](https://map.meshcore.dev) ([bot docs](https://github.com/agessaman/meshcore-bot/blob/main/docs/map-uploader.md)): default **off**; infra ADVERTs with GPS only (repeaters/rooms/sensors); skip CHAT/companions, Tracker trails, site pins; rate-limit re-uploads; private-key signing operator-gated

## Phase F — USB + multi-radio pool + TCP

BLE, USB serial, and TCP (Wi-Fi / Ethernet companions) share one radio pool. Cookbook USB/serial patterns: [agessaman/meshcore-bot](https://github.com/agessaman/meshcore-bot). One HQ, several attached MeshCore devices (e.g. companion + room server + repeater).

- [x] **F1** USB serial transport beside BLE: discover ports, connect/disconnect, same session pipeline (appstart → contacts → drain → subscribe); reuse `meshcore-rs` USB path where available; study bot USB code for quirks
- [x] **F2** Multi-radio pool: N concurrent radio tasks (one per device); shared SQLite; UI shows each radio with transport (BLE/USB/TCP), name, battery/role, connect state
- [x] **F3** Operator roles per radio: label as companion / room server / repeater (and similar); clarify which radio owns METIS-INFO TX, enrollment, and bot replies (default: primary companion)
- [x] **F4** Contact/message ingest tagged by `radio_id`; map + roster stay unified; avoid double-counting the same pubkey heard on two HQ radios
- [x] **F5** Settings: pick primary radio (`primary_radio_id`); reconnect-with-backoff unless the operator disconnected
- [x] **F5b** USB prefer sticky port/serial identity when the OS renumerates (optional polish)
- [x] **F6** TCP companion (Wi-Fi / Ethernet): same session pool as BLE/USB; Settings host + port (default **5000**); Ethernet companions use this path too; no mDNS / Wi-Fi credential wizard in the first cut

## Phase G — Offline firmware locker + USB flasher

Mirror MeshCore community firmware while online; flash over USB when the internet is gone.

- [x] **G1** Firmware locker: fetch flasher `config.json` + GitHub companion/repeater/room-server releases (latest + previous); store under `data/firmware/`; index + prune
- [x] **G2** Background sync (12h / Sync now / enable toggle); offline UI still flashes from cache
- [x] **G3** Settings → Flasher: device/role/version/port; ESP32 via esptool; nRF52 via adafruit-nrfutil or UF2 path; erase off by default
- [x] **G4** Selective locker + Manage overlay: `selection.json`, Download-all (~1 GB), Latest/Previous, fuzzy/zebra/pills; sync downloads only the checked set and prunes orphans

## Phase H — Standalone desktop ship

HQ must run offline as a normal desktop app on operator laptops — no Docker, no cloud runtime. One program per OS.

- [x] **H1** Standalone executables for **Linux**, **Windows**, and **macOS** (`dx` / Cargo desktop release builds); ship with bundled MapLibre assets; document install + first-run `data/` location
- [x] **H1b** First-run welcome wizard (critical/soft deps install + optional companion connect) and Settings → About (version, author, MIT, GitHub, data path)
- [ ] **H2** Smoke each target: window opens, SQLite creates under `data/`, BLE/USB paths degrade cleanly when hardware is absent
- [ ] **H3** SQLite durability: confirm **WAL** journal mode on open (already set in `Db::open`); keep WAL + checkpoint behavior healthy under multi-radio ingest so the DB does not choke or lock up; note `-wal`/`-shm` beside the main file in backup docs

## Phase I — Infrastructure (claimed repeaters / rooms / sensors)

Heard nodes stay on Roster/Map. **Claimed** nodes are HQ-owned mesh infrastructure. The companion remains the radio; claimed nodes are LoRa targets via `run_cli` / remote CLI — never treat repeater firmware as a pool companion.

- [x] **I1** Claim store: `claimed_infra` (pubkey, kind, label, notes, added_at, verified_at); kind from contact_type (2→repeater, 3→room, 4→sensor) with override; **no admin passwords** in SQLite
- [x] **I2** Infrastructure page: list/detail, claim from roster, unclaim, notes; status probe (rate-limited `ver` / `get radio` / `neighbors`) + remote CLI presets through primary companion
- [x] **I3** Overview strip **only when claims exist** (count + recent/stale); no empty chrome for unclaimed heard repeaters
- [x] **I4** Roster Claim / Claimed badge for types 2/3/4
- [x] **I5** Verify-on-claim: `CMD_SEND_LOGIN` + admin ACL proof via primary companion; fail closed (guest/wrong password never inserts); day-to-day stats use node ACL, re-login only when ACL expires

## Phase J — Nav / list UX

- [x] **J1** Sidebar order by use: Overview → Messages → Inbox → Roster → Map → Infrastructure → Tracker → CLI; Settings stays bottom
- [x] **J2** Expanded-only fuzzy jump: pages + in-page tabs/keywords (not live roster/message bodies)
- [x] **J3** Zebra rows on Roster, Infrastructure (claimed + claim-from-roster), Tracker (tracked + add-from-roster) via `.row-zebra` in `form.css` (frozen Tailwind lacks `bg-muted/40`)
- [x] **J4** GPS locate icon: green within 7 days of `last_heard`, red when older / never
- [x] **J5** Shell: opaque top banner + `min-h-0` / `overflow-hidden` so page content does not paint under the bar

## Non-goals (for now)

- Potato federation / multi-ingestor dashboard
- Merging meshes or bridging GPS tracks
- Continuous cellular-style tracking
- Auto-adding every public advert onto the radio contact table
- Stamping Open-Meteo/USGS URLs on mesh text (devices are plain text)

## Suggested order

1. A1 → A2  
2. B1 → B4  
3. A3 → B5  
4. C  
5. D1 → D3  
6. D4 · D1c (local MQTT station vs Open-Meteo) — done  

7. F1 → F3 (USB + multi-radio; primary companion owns bot)  
8. E1 → E3 (backup first), then E4 / F4–F5b polish  
9. G1 → G4 (offline firmware locker + selective Manage overlay + USB flasher)  
10. H1 → H3 (standalone Linux/Windows/macOS + WAL-safe SQLite)  
11. I1 → I5 (claimed infrastructure + verify-on-claim + gated Overview)  
12. J1 → J5 (nav order, fuzzy jump, zebra, heard-week icon, shell layout)

## Success checks

- [x] Shared-GPS mover on whitelist: pin updates, trail grows
- [x] Whitelisted but not sharing: last-known + stale, no fake motion
- [x] Message from mapped node: two rings, no pan jank
- [x] First-seen coords: warp-in once (not every refresh)
- [x] Weather/quake: compact, sourced, enrolled-only, audited
- [x] MQTT station fresh → mesh WX uses it; stale/down → Open-Meteo fallback
- [x] Ecowitt push/pull → Weather page + bare #weather; no cloud station APIs
- [x] USB companion + BLE/USB/TCP second radio both live under one HQ; bot TX from primary only
- [x] Backup restores roster + secrets on a fresh machine; import does not flood radio contacts
- [x] Map uploader (if on): only infra GPS adverts; companions/tracks stay local
- [x] Release artifacts: one standalone binary (or app bundle) each for Linux, Windows, macOS
- [ ] Fresh launch: `PRAGMA journal_mode` reports `wal`; app survives concurrent radio writes without SQLite lock storms
- [x] Claim repeater → Infrastructure page + Overview strip; probe/CLI via linked companion; empty claim list hides Overview infra chrome
- [x] Claim requires admin login proof; guest/wrong password never inserts; Unclaim then reclaim asks for password again
- [x] Expanded nav search finds Settings for “device” / tab keywords; Roster zebra rows visible; pages clear the top banner

