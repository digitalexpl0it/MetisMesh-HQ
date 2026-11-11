#!/usr/bin/env bash
# Build a portable Linux AppImage from target/release/metis-mesh-hq.
#
# Bundles GTK (via linuxdeploy-plugin-gtk) but intentionally does **not**
# ship WebKitGTK / JavaScriptCore. Ubuntu-built libwebkit2gtk hardcodes
# Debian multiarch helper paths (/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/…)
# and ignores WEBKIT_EXEC_PATH — so Arch/CachyOS/Fedora crash even when
# helpers are copied into the AppImage (GitHub issue #1). Host webkit2gtk-4.1
# works; that matches the confirmed CachyOS workaround.
#
# Host still needs: webkit2gtk-4.1 (+ JSC), BlueZ, dialout/udev for USB.
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

EXCLUDE_WEBKIT=(
  --exclude-library '*webkit2gtk*'
  --exclude-library '*javascriptcoregtk*'
  --exclude-library '*WebKit*'
)

# Phase 1: populate AppDir (GTK plugin + deps) without packing yet.
"$linuxdeploy" \
  --appdir "$APPDIR" \
  --executable "$APPDIR/usr/bin/metis-mesh-hq" \
  --desktop-file "$APPDIR/usr/share/applications/metis-mesh-hq.desktop" \
  --icon-file "$APPDIR/usr/share/icons/hicolor/256x256/apps/metis-mesh-hq.png" \
  --plugin gtk \
  "${EXCLUDE_WEBKIT[@]}"

# Belt-and-suspenders: remove any WebKit/JSC that still landed in AppDir.
stripped=0
while IFS= read -r -d '' path; do
  rm -f "$path"
  stripped=$((stripped + 1))
done < <(find "$APPDIR" \( \
  -name 'libwebkit2gtk*.so*' -o \
  -name 'libjavascriptcoregtk*.so*' -o \
  -name 'WebKitNetworkProcess' -o \
  -name 'WebKitWebProcess' -o \
  -name 'WebKitGPUProcess' \
  \) -print0 2>/dev/null || true)
if [[ "$stripped" -gt 0 ]]; then
  echo "removed $stripped leftover WebKit/JSC file(s) from AppDir (use host webkit2gtk-4.1)"
fi
rmdir "$APPDIR/usr/lib/webkit2gtk-4.1" 2>/dev/null || true
rmdir "$APPDIR/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1" 2>/dev/null || true

# Phase 2: pack cleaned AppDir (excludes again so a dep rescan cannot re-add WebKit).
"$linuxdeploy" \
  --appdir "$APPDIR" \
  --executable "$APPDIR/usr/bin/metis-mesh-hq" \
  --desktop-file "$APPDIR/usr/share/applications/metis-mesh-hq.desktop" \
  --icon-file "$APPDIR/usr/share/icons/hicolor/256x256/apps/metis-mesh-hq.png" \
  "${EXCLUDE_WEBKIT[@]}" \
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

# Fail the build if WebKit still slipped into the published image.
verify_dir="$(mktemp -d)"
cleanup_verify() { rm -rf "$verify_dir"; }
trap cleanup_verify EXIT
OUT_ABS="$(realpath "$OUT_NAME")"
(
  cd "$verify_dir"
  APPIMAGE_EXTRACT_AND_RUN=1 "$OUT_ABS" --appimage-extract >/dev/null
)
if find "$verify_dir" \( \
  -name 'libwebkit2gtk*.so*' -o \
  -name 'libjavascriptcoregtk*.so*' \
  \) | grep -q .; then
  echo "ERROR: AppImage still contains bundled WebKit/JSC (GH #1 will recur)" >&2
  find "$verify_dir" \( -name 'libwebkit2gtk*.so*' -o -name 'libjavascriptcoregtk*.so*' \) >&2
  exit 1
fi
echo "verified: no bundled libwebkit2gtk / libjavascriptcoregtk in $OUT_NAME"
ls -lh "$OUT_NAME"
