# Metis Mesh HQ

Local emergency headquarters for [MeshCore](https://meshcore.co.uk/) communities.

Churches, neighborhoods, and volunteer groups run this on a regular computer sitting next to a MeshCore companion radio. It keeps working when the internet drops. No cloud account, and it does not call home.

## What it does

- Talks to MeshCore companions over **Bluetooth**, **USB serial**, or **TCP** (Wi-Fi / Ethernet, default port 5000) on one radio pool
- Enrolls devices, hands out private channel secrets, and runs a local bot
- Keeps a roster of everyone heard on the mesh, whether they enrolled or not
- Claims and admin-verifies infrastructure (repeaters / rooms / sensors) over LoRa through the primary companion
- Maps nodes with MapLibre + a local style (and optional PMTiles offline packs)
- Tracks an operator whitelist of devices expected to share GPS (Tracker)
- Offline firmware locker + USB flasher (checksums, Ready-for-outage status, export/import `.zip` pack); encrypted HQ backup/restore
- Volunteer offline runbook (`docs/OFFLINE_RUNBOOK.md` and Settings → About)
- Stores messages and contacts in SQLite under the per-user data folder
- Puts channel secrets in the OS keyring when it can; otherwise a locked-down file

Communities stay separate until both headquarters agree to link. That bridge is not in this MVP yet — the data model leaves room for it.

## Install (standalone binary)

GitHub Actions builds Ubuntu 26.04 (`x86_64`), Windows (`x86_64`), and macOS (`aarch64`) on every push to `main` and on version tags. Linux support is Ubuntu 26.04+ only.

1. Download the archive for your OS from a [GitHub Release](https://github.com/digitalexpl0it/MetisMesh-HQ/releases) (preferred), or from the Actions tab artifacts on `main`.
2. Unpack into a folder you own (for example `~/metis-hq/` or `C:\MetisHQ\`).
3. Run the binary from that folder:
   - Ubuntu / macOS: `./metis-mesh-hq-ubuntu-26.04-x86_64` (or the macOS name from the archive)
   - Windows: double-click or run `metis-mesh-hq-windows-x86_64.exe`
4. First launch creates the per-user data folder (SQLite, firmware cache, backups, QR files):
   - Linux: `~/.MetisMeshHQ`
   - macOS: `~/Library/Application Support/MetisMeshHQ`
   - Windows: `%APPDATA%\MetisMeshHQ`
   
   Override with `METIS_DATA_DIR=/path/to/dir` if you want the data elsewhere.

5. First launch opens a **setup wizard**: checks Bluetooth / USB serial access, can install missing pieces (may prompt for admin / `pkexec`), and optionally connects a companion (BLE / USB / TCP). You can skip and finish later under Settings → Connect. Settings → About has version, license, GitHub, and data-folder path; **Run setup again** reopens the wizard.

### Runtime libraries

Critical UI libraries are probed before the window opens. On Debian/Ubuntu the app will try `pkexec apt-get install` for WebKitGTK/GTK if they are missing; on Windows it can download the WebView2 Evergreen bootstrapper. You can still install manually:

- **Linux (Ubuntu 26.04+):**

  ```bash
  sudo apt-get update
  sudo apt-get install -y \
    libwebkit2gtk-4.1-0 libgtk-3-0t64 \
    libayatana-appindicator3-1 libsoup-3.0-0 \
    libjavascriptcoregtk-4.1-0 libdbus-1-3
  ```

  Bluetooth needs BlueZ. USB serial needs membership in `dialout` (log out/in after the wizard adds you).

- **Windows:** Edge WebView2 runtime (usually already present on current Windows 10/11). If missing, HQ downloads the Evergreen bootstrapper with PowerShell and runs a silent install. Optional flashers (`esptool`, `adafruit-nrfutil`) install the same way (winget Python if needed, then `pip --user`).
- **macOS:** system WebKit (no extra install). First open may need right-click → Open if Gatekeeper blocks an unsigned binary.

### Publishing a GitHub Release

Releases are created only from version tags (not every `main` push):

```bash
git tag v0.1.0
git push origin v0.1.0
```

That runs the Build workflow and attaches `metis-mesh-hq-*-v0.1.0.tar.gz` / `.zip` to the new release.

## Requirements (build from source)

- Rust (1.88+ is fine; we build on stable)
- [Dioxus CLI](https://dioxuslabs.com/) (`dx`) for day-to-day runs
- A MeshCore companion radio nearby
- On Linux: WebKitGTK 4.1, GTK 3, D-Bus, and libudev headers (see below)

### Linux packages (Ubuntu 26.04+)

Supported Linux baseline is **Ubuntu 26.04** (and newer). Build deps:

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential pkg-config \
  libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev libxdo-dev \
  libsoup-3.0-dev libjavascriptcoregtk-4.1-dev \
  libdbus-1-dev libudev-dev
```

Bluetooth needs BlueZ running. Disconnect the MeshCore phone app before HQ connects over BLE — the companion only allows one GATT client. USB and TCP do not use a companion PIN.

## Run it

```bash
cargo install dioxus-cli   # once
dx serve --platform desktop
```

Or:

```bash
cargo run --release
```

On first launch it creates the per-user data folder (`~/.MetisMeshHQ` on Linux — see Install) for the database and QR cache.

## Build releases yourself

```bash
cargo build --release
```

Binary lands at `target/release/metis-mesh-hq` (`.exe` on Windows). MapLibre and other `assets/` files are embedded via Dioxus `asset!()`.

## Layout

| Path | What |
| --- | --- |
| `src/` | App, radio task, SQLite, map tiles, UI |
| `assets/` | MapLibre, CSS, presets, offline map bits |
| profile data dir | Local DB + QR SVGs (`~/.MetisMeshHQ` on Linux; not in git) |
| `.github/workflows/` | Cross-platform builds |

## Notes worth knowing

- Mesh text is plain text, max 160 bytes. Never treat it like a shell or HTML.
- Only enrolled nodes get bot replies or channel secrets.
- GPS from a node is self-reported. An enrolled site pin is separate.
- This is a community tool. It is not FEMA, NWS, or law enforcement.

## License

MIT — see `Cargo.toml`.
