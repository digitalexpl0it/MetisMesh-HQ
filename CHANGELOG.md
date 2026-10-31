# Changelog

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

Still ahead: HQ-to-HQ linking, USB companions on the same pool, and packaging polish (signed installers, etc.).
