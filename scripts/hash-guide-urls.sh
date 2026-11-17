#!/usr/bin/env bash
# Fetch guide pack URLs and print sha256 + size for catalog.seed.json.
# Usage:
#   ./scripts/hash-guide-urls.sh [url ...]
#   ./scripts/hash-guide-urls.sh   # reads files[].url from assets/guides/catalog.seed.json
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# Keep in sync with src/guides.rs USER_AGENT (no "Metis" — Ready.gov/Akamai 403).
UA='Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36 LocalHQ/0.1'

urls=("$@")
if [[ ${#urls[@]} -eq 0 ]]; then
  seed="$ROOT/assets/guides/catalog.seed.json"
  if [[ ! -f "$seed" ]]; then
    echo "No URLs given and missing $seed" >&2
    exit 1
  fi
  mapfile -t urls < <(python3 - <<'PY' "$seed"
import json, sys
data = json.load(open(sys.argv[1]))
for pack in data.get("packs", []):
    for f in pack.get("files") or []:
        u = (f.get("url") or "").strip()
        if u:
            print(u)
    u = (pack.get("download_url") or "").strip()
    if u:
        print(u)
PY
)
fi

if [[ ${#urls[@]} -eq 0 ]]; then
  echo "No URLs to hash." >&2
  exit 1
fi

for u in "${urls[@]}"; do
  echo "=== $u"
  tmp="$(mktemp)"
  ref_args=()
  case "$u" in
    *ready.gov*) ref_args=(-H 'Referer: https://www.ready.gov/') ;;
    *gutenberg.org*|*pglaf.org*) ref_args=(-H 'Referer: https://www.gutenberg.org/') ;;
  esac
  accept='*/*'
  case "$u" in
    *.pdf|*.PDF) accept='application/pdf,*/*;q=0.8' ;;
  esac
  if curl -fsSL -L -A "$UA" -H "Accept: $accept" "${ref_args[@]}" --max-time 180 -o "$tmp" "$u"; then
    sz="$(wc -c <"$tmp" | tr -d ' ')"
    sha="$(sha256sum "$tmp" | awk '{print $1}')"
    echo "type: $(file -b "$tmp" | tr '\n' ' ')"
    echo "size_bytes: $sz"
    echo "sha256: $sha"
  else
    echo "FAILED (check URL or User-Agent / CDN block)" >&2
  fi
  rm -f "$tmp"
  echo
done
