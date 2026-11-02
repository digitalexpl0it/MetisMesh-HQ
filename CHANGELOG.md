# Changelog

## Unreleased — 2026-11-02

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

### UX

- Nav order by use (Overview → Messages → Inbox → Roster → Map → Infrastructure → Tracker → CLI; Settings bottom)
- Expanded-nav fuzzy jump (pages + tab/keyword index)
- Zebra rows on Roster / Infrastructure / Tracker (`.row-zebra` in `form.css`)
- GPS locate icon green within a week of last heard, red when older
- Shell layout: opaque top bar + flex overflow so pages do not paint under the banner

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
