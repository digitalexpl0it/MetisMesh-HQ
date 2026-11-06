# Offline runbook — first hour without Internet

> **Canonical copy for operators:** open **Help → Offline runbook** inside Metis Mesh HQ.
> This file mirrors that in-app topic for GitHub / print-from-source. Standalone installs do not ship a separate markdown viewer.

One page for church / community volunteers running **Metis Mesh HQ** when the WAN is down. Prefer the in-app Help page on the HQ laptop.

## Before the outage (do this while online)

1. Install Metis Mesh HQ and finish the setup wizard (Bluetooth / USB dialout / optional flashers).
2. Connect the MeshCore companion radio (BLE, USB, or TCP).
3. **Settings → Flasher → Manage firmware**: select the boards and roles you actually own → **Save & sync**.
4. Wait until the Flasher page says **Ready for outage**. Optionally **Check integrity**, then **Export pack** to a `MetisMeshHQ-firmware-*.zip` on a USB stick.
5. Leave the laptop plugged into a UPS or wall power; disable sleep while HQ is the desk radio.

## First hour offline

1. **Power** — laptop on AC or UPS; companion radio powered. Do not let the OS sleep.
2. **Open HQ** — start Metis Mesh HQ from the desktop / Start menu (no cloud login).
3. **Radio** — Settings → Connect: confirm the companion shows connected. If USB moved ports, Refresh ports and reconnect.
4. **Map** — open Map; confirm local pins / last-heard nodes (self-reported GPS only).
5. **Channel** — open Messages → Channels; post a short check-in on your community channel so volunteers know HQ is live. Speakers who post appear under the channel for this session only.
6. **Roster / Contacts** — approve pending enrollments only for people you trust; secrets are never shown in the log. DMs need a contact card or companion contact key — not a channel display name alone.
7. **Flash only from cache** — Settings → Flasher. Flash is blocked if a file fails checksum. If the locker is empty, **Import pack** and pick the USB `.zip`.

## What the HQ computer needs on battery / UPS

| Need | Guidance |
|------|----------|
| Runtime | Size the UPS for laptop + USB radio (often 2–4 hours minimum for a desk shift). |
| Sleep | Turn off suspend/hibernate while HQ is on duty. |
| Display | Dim the screen to save power; keep the window open so status stays visible. |
| Network | Internet is optional once firmware is synced. Mesh traffic is radio-local. |
| Heat / dust | Keep vents clear; a closed closet kills laptops on UPS. |

## Do / don’t

- **Do** treat HQ as the local system of record (roster, map, bot, audit log).
- **Do** flash only indexed, verified firmware from this machine’s locker.
- **Don’t** present HQ messages as FEMA, NWS, or law enforcement.
- **Don’t** share channel secrets in chat or screenshots.

## Help

Settings → About shows version and the data folder path (`~/.MetisMeshHQ` on Linux). The live database is `metis.db` with optional `metis.db-wal` / `metis.db-shm` sidecars — use Settings → Backup for a consistent copy, or include all three if you copy the folder cold. Source and issues: https://github.com/digitalexpl0it/MetisMesh-HQ
