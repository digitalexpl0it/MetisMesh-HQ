#!/usr/bin/env bash
# Build an amd64 .deb for Ubuntu 26.04+ from target/release/metis-mesh-hq.
# Uses system WebKit/GTK (Depends:). AppImage is the portable alternative.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

BINARY="${BINARY:-target/release/metis-mesh-hq}"
VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
ARCH="${ARCH:-amd64}"
OUT_NAME="${OUT_NAME:-metis-mesh-hq_${VERSION}_${ARCH}.deb}"
STAGE="${STAGE:-/tmp/metis-deb-stage}"

if [[ ! -x "$BINARY" ]]; then
  echo "missing executable: $BINARY (build with cargo build --release first)" >&2
  exit 1
fi
if ! command -v dpkg-deb >/dev/null; then
  echo "dpkg-deb is required (apt install dpkg-dev)" >&2
  exit 1
fi

rm -rf "$STAGE"
pkg="$STAGE/metis-mesh-hq_${VERSION}_${ARCH}"
mkdir -p \
  "$pkg/DEBIAN" \
  "$pkg/usr/bin" \
  "$pkg/usr/lib/metis-mesh-hq" \
  "$pkg/usr/share/applications" \
  "$pkg/usr/share/icons/hicolor/256x256/apps" \
  "$pkg/usr/share/doc/metis-mesh-hq"

install -m 755 "$BINARY" "$pkg/usr/lib/metis-mesh-hq/metis-mesh-hq"
cp -a assets "$pkg/usr/lib/metis-mesh-hq/assets"
# PATH launcher — real binary + assets live together under /usr/lib.
cat > "$pkg/usr/bin/metis-mesh-hq" <<'EOF'
#!/bin/sh
exec /usr/lib/metis-mesh-hq/metis-mesh-hq "$@"
EOF
chmod 755 "$pkg/usr/bin/metis-mesh-hq"

install -m 644 packaging/linux/metis-mesh-hq.desktop \
  "$pkg/usr/share/applications/metis-mesh-hq.desktop"
install -m 644 assets/app_icon_512.png \
  "$pkg/usr/share/icons/hicolor/256x256/apps/metis-mesh-hq.png"

if [[ -f LICENSE ]]; then
  install -m 644 LICENSE "$pkg/usr/share/doc/metis-mesh-hq/copyright"
elif [[ -f LICENSE.md ]]; then
  install -m 644 LICENSE.md "$pkg/usr/share/doc/metis-mesh-hq/copyright"
fi
if [[ -f CHANGELOG.md ]]; then
  gzip -9n -c CHANGELOG.md > "$pkg/usr/share/doc/metis-mesh-hq/changelog.gz"
fi

# Ubuntu 26.04 package names (libgtk-3-0t64). Older Ubuntu may need a rebuild.
cat > "$pkg/DEBIAN/control" <<EOF
Package: metis-mesh-hq
Version: ${VERSION}
Section: net
Priority: optional
Architecture: ${ARCH}
Maintainer: DigitalExpl0it <https://github.com/digitalexpl0it/MetisMesh-HQ>
Homepage: https://github.com/digitalexpl0it/MetisMesh-HQ
Depends: libwebkit2gtk-4.1-0, libgtk-3-0t64, libayatana-appindicator3-1, libsoup-3.0-0, libjavascriptcoregtk-4.1-0, libdbus-1-3, libudev1
Recommends: bluez
Description: Offline emergency headquarters for MeshCore communities
 Local desktop HQ for MeshCore companion radios (BLE, USB, TCP).
 Enrollment, messaging, map, firmware locker, and backups with no cloud.
 .
 Built for Ubuntu 26.04+. Other distros should prefer the AppImage.
EOF

# Installed-Size in KiB
size_kib=$(du -sk "$pkg" | awk '{print $1}')
echo "Installed-Size: ${size_kib}" >> "$pkg/DEBIAN/control"

chmod 755 "$pkg/DEBIAN"
find "$pkg" -type d -exec chmod 755 {} +
# Control files must not be group/world writable for dpkg-deb.
chmod -R u=rwX,go=rX "$pkg/DEBIAN"

dpkg-deb --root-owner-group --build "$pkg" "$OUT_NAME"
ls -lh "$OUT_NAME"
dpkg-deb --info "$OUT_NAME"
