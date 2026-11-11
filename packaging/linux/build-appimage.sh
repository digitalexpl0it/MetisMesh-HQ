#!/usr/bin/env bash
# Build a portable Linux AppImage from target/release/metis-mesh-hq.
# Bundles GTK/WebKit shared libs via linuxdeploy so the image runs on
# modern x86_64 desktops beyond Ubuntu 26.04. BlueZ / udev stay on the host.
#
# WebKitGTK also needs its helper processes (Network/Web/GPU). Shipping the
# .so files alone still looks for Debian paths like
# /usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/WebKitNetworkProcess and aborts on
# Arch/CachyOS (GitHub issue #1). We copy those helpers and set WEBKIT_EXEC_PATH.
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

# Locate WebKitGTK helper processes on the build host (Ubuntu CI path first).
helper_src=""
multiarch="$(dpkg-architecture -qDEB_HOST_MULTIARCH 2>/dev/null || true)"
for cand in \
  ${multiarch:+"/usr/lib/${multiarch}/webkit2gtk-4.1"} \
  /usr/lib/x86_64-linux-gnu/webkit2gtk-4.1 \
  /usr/lib64/webkit2gtk-4.1 \
  /usr/lib/webkit2gtk-4.1; do
  if [[ -d "$cand" ]]; then
    helper_src="$cand"
    break
  fi
done
if [[ -z "$helper_src" ]]; then
  echo "WebKitGTK 4.1 helper directory not found (install libwebkit2gtk-4.1-0 / -dev)." >&2
  exit 1
fi

helper_dst="$APPDIR/usr/lib/webkit2gtk-4.1"
mkdir -p "$helper_dst"
helper_exec_args=()
for bin in WebKitNetworkProcess WebKitWebProcess WebKitGPUProcess; do
  if [[ -x "$helper_src/$bin" ]]; then
    cp -a "$helper_src/$bin" "$helper_dst/$bin"
    chmod +x "$helper_dst/$bin"
    helper_exec_args+=(--executable "$helper_dst/$bin")
    echo "bundled WebKit helper: $bin"
  fi
done
if [[ ${#helper_exec_args[@]} -eq 0 ]]; then
  echo "no WebKit helper executables in $helper_src" >&2
  exit 1
fi

# AppRun sources every script under apprun-hooks/ (after extraction / in place).
mkdir -p "$APPDIR/apprun-hooks"
cat > "$APPDIR/apprun-hooks/zz-metis-webkit.sh" <<'EOF'
# Use WebKit helpers shipped inside this AppImage instead of hard-coded
# Debian multiarch paths (fixes Arch / CachyOS startup — GH #1).
APPDIR="${APPDIR:-$(dirname "$(realpath "$0")")}"
export WEBKIT_EXEC_PATH="${APPDIR}/usr/lib/webkit2gtk-4.1"
EOF
chmod +x "$APPDIR/apprun-hooks/zz-metis-webkit.sh"

# CI runners often lack FUSE; extract-and-run works without it.
export APPIMAGE_EXTRACT_AND_RUN=1
export LINUXDEPLOY_OUTPUT_VERSION="${LINUXDEPLOY_OUTPUT_VERSION:-}"
export DEPLOY_GTK_VERSION=3

"$linuxdeploy" \
  --appdir "$APPDIR" \
  --executable "$APPDIR/usr/bin/metis-mesh-hq" \
  "${helper_exec_args[@]}" \
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
