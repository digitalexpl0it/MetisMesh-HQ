#!/usr/bin/env bash
# Sync Screenshots + brand marks into site/media for Cloudflare Pages / local preview.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DEST="$ROOT/site/media"

mkdir -p "$DEST"

cp -a "$ROOT/Screenshots/." "$DEST/"

for f in emerghq_logo.png mmhq_badge.png app_icon_512.png app_icon.png; do
  if [[ -f "$ROOT/assets/$f" ]]; then
    cp -a "$ROOT/assets/$f" "$DEST/$f"
  fi
done

# Favicon from app icon when present
if [[ -f "$DEST/app_icon.png" ]]; then
  cp -a "$DEST/app_icon.png" "$DEST/favicon.png"
fi

echo "site/media ready ($(find "$DEST" -type f | wc -l) files)"
