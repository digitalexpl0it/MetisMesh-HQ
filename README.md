# Metis Mesh HQ

Local emergency headquarters for [MeshCore](https://meshcore.co.uk/) communities.

Churches, neighborhoods, and volunteer groups run this on a laptop or a LattePanda next to a MeshCore companion radio. It keeps talking when the internet is gone. No cloud account. No phone home.

## What it does

- Talks to a MeshCore companion over Bluetooth (USB is on the same path later)
- Enrolls devices, hands out private channel secrets, and runs a local bot
- Keeps a roster of everyone heard on the mesh, whether they enrolled or not
- Maps nodes with MapLibre + a local style (and optional PMTiles offline packs)
- Stores messages and contacts in SQLite under `data/`
- Puts channel secrets in the OS keyring when it can; otherwise a locked-down file

Communities stay separate until both headquarters agree to link. That bridge is not in this MVP yet — the data model leaves room for it.

## Requirements

- Rust (1.88+ is fine; we build on stable)
- [Dioxus CLI](https://dioxuslabs.com/) (`dx`) for day-to-day runs
- A MeshCore companion radio nearby
- On Linux: WebKitGTK 4.1, GTK 3, and D-Bus headers (see below)

### Linux packages (Debian / Ubuntu)

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential pkg-config \
  libwebkit2gtk-4.1-dev libgtk-3-dev \
  libayatana-appindicator3-dev libxdo-dev \
  libsoup-3.0-dev libjavascriptcoregtk-4.1-dev \
  libdbus-1-dev
```

Bluetooth needs BlueZ running. Disconnect the MeshCore phone app before HQ connects — the companion only allows one BLE client.

## Run it

```bash
cargo install dioxus-cli   # once
dx serve --platform desktop
```

Or:

```bash
cargo run --release
```

On first launch it creates `data/` (gitignored) for the database and QR cache.

## Build releases yourself

```bash
cargo build --release
```

Binary lands at `target/release/metis-mesh-hq` (`.exe` on Windows).

GitHub Actions builds Linux, Windows, and macOS on every push to `main` and on tags. Grab the artifacts from the Actions tab, or from a release if you tagged one (`v0.1.0`, etc.).

## Layout

| Path | What |
| --- | --- |
| `src/` | App, radio task, SQLite, map tiles, UI |
| `assets/` | MapLibre, CSS, presets, offline map bits |
| `data/` | Local DB + QR SVGs (not in git) |
| `.github/workflows/` | Cross-platform builds |

## Notes worth knowing

- Mesh text is plain text, max 160 bytes. Never treat it like a shell or HTML.
- Only enrolled nodes get bot replies or channel secrets.
- GPS from a node is self-reported. An enrolled site pin is separate.
- This is a community tool. It is not FEMA, NWS, or law enforcement.

## License

MIT — see `Cargo.toml`.
