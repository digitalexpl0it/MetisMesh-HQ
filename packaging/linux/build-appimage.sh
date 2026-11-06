#!/usr/bin/env bash
# Build a portable Linux AppImage from target/release/metis-mesh-hq.
# Bundles GTK/WebKit shared libs via linuxdeploy so the image runs on
# modern x86_64 desktops beyond Ubuntu 26.04. BlueZ / udev stay on the host.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

BINARY="${BINARY:-target/release/metis-mesh-hq}"
OUT_NAME="${OUT_NAME:-metis-mesh-hq-x86_64.AppImage}"
APPDIR="${APPDIR:-AppDir}"
TOOLS="${TOOLS:-/tmp/metis-linuxdeploy}"

if [[ ! -x "$BINARY" ]]; then
  echo "missing executable: $BINARY (build with cargo build --release first)" >&2
  exit 1
fi

mkdir -p "$TOOLS"
linuxdeploy="$TOOLS/linuxdeploy-x86_64.AppImage"
# linuxdeploy-plugin-gtk has no AppImage release — use the upstream shell plugin.
gtk_plugin="$TOOLS/linuxdeploy-plugin-gtk"
if [[ ! -x "$linuxdeploy" ]]; then
  curl -fsSL -o "$linuxdeploy" \
    "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage"
  chmod +x "$linuxdeploy"
fi
if [[ ! -x "$gtk_plugin" ]]; then
  curl -fsSL -o "$gtk_plugin" \
    "https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh"
  chmod +x "$gtk_plugin"
fi
# linuxdeploy finds plugins named linuxdeploy-plugin-<name> on PATH.
export PATH="$TOOLS:$PATH"

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/applications" \
  "$APPDIR/usr/share/icons/hicolor/256x256/apps" \
  "$APPDIR/usr/share/metis-mesh-hq"

cp "$BINARY" "$APPDIR/usr/bin/metis-mesh-hq"
chmod +x "$APPDIR/usr/bin/metis-mesh-hq"
# Backup for any remaining dioxus asset!() URLs; map scripts are embedded.
cp -a assets "$APPDIR/usr/bin/assets"
cp packaging/linux/metis-mesh-hq.desktop "$APPDIR/usr/share/applications/"
cp assets/app_icon_512.png "$APPDIR/usr/share/icons/hicolor/256x256/apps/metis-mesh-hq.png"

# CI runners often lack FUSE; extract-and-run works without it.
export APPIMAGE_EXTRACT_AND_RUN=1
export LINUXDEPLOY_OUTPUT_VERSION="${LINUXDEPLOY_OUTPUT_VERSION:-}"
export DEPLOY_GTK_VERSION=3

"$linuxdeploy" \
  --appdir "$APPDIR" \
  --executable "$APPDIR/usr/bin/metis-mesh-hq" \
  --desktop-file "$APPDIR/usr/share/applications/metis-mesh-hq.desktop" \
  --icon-file "$APPDIR/usr/share/icons/hicolor/256x256/apps/metis-mesh-hq.png" \
  --plugin gtk \
  --output appimage

# linuxdeploy names the image from the desktop Name= field.
shopt -s nullglob
built=(Metis_Mesh_HQ*.AppImage)
if [[ ${#built[@]} -eq 0 ]]; then
  built=(Metis*.AppImage)
fi
if [[ ${#built[@]} -eq 0 ]]; then
  echo "linuxdeploy did not produce an AppImage" >&2
  ls -la ./*.AppImage 2>/dev/null || true
  exit 1
fi
mv -f "${built[0]}" "$OUT_NAME"
for extra in "${built[@]:1}"; do
  rm -f "$extra"
done
ls -lh "$OUT_NAME"
