# Metis Mesh HQ — map presence, Tracker, services

Game plan from potato-mesh (sonar UX), meshcore-bot (weather/quake), and Tracker whitelist.
Build in order; check boxes as we finish.

## Principles

- Local HQ owns the radio; no second BLE client.
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

- [x] **D1** Weather: Open-Meteo on **METIS-INFO** (`#weather [zip]`); enrolled-only; 45s rate-limit; ≤160 bytes
- [x] **D2** NWS alerts: poll near HQ site pin; severity + is.gd short URL; Inbox confirm unless auto-relay → METIS-INFO
- [x] **D3** Earthquakes: USGS bbox (default CA) + magnitude + dedupe; `#quake` + poll posts on METIS-INFO
- [x] HQ-owned `services` module + poller (not a second companion). DMs stay signup/status only.
- [ ] **D4** Separate private peer channel (e.g. METIS-NET) for human emergency traffic (METIS-INFO = bot/info)

## Non-goals (for now)

- Potato federation / multi-ingestor dashboard
- Merging meshes or bridging GPS tracks
- Continuous cellular-style tracking
- Auto-adding every public advert onto the radio contact table

## Suggested order

1. A1 → A2  
2. B1 → B4  
3. A3 → B5  
4. C  
5. D1 → D3  

## Success checks

- [x] Shared-GPS mover on whitelist: pin updates, trail grows
- [x] Whitelisted but not sharing: last-known + stale, no fake motion
- [x] Message from mapped node: two rings, no pan jank
- [x] First-seen coords: warp-in once (not every refresh)
- [x] Weather/quake: compact, sourced, enrolled-only, audited
