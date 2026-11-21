# Metis Mesh HQ — marketing site

Static dark landing page for Cloudflare (`mmhq.digitalexpl0it.com`).

## Local preview

From the repo root:

```bash
bash scripts/prepare-site-media.sh
cd site && python3 -m http.server 8787
```

Open `http://127.0.0.1:8787`.

## Cloudflare (current dashboard)

Cloudflare’s UI now folds this into **Workers & Pages → Create application** (Git import). You will **not** see a separate “Build output directory” field — only Build / Deploy / Preview commands. That is expected.

1. **Workers & Pages** → **Create application**.
2. Import the GitHub repo `digitalexpl0it/MetisMesh-HQ` (Workers + static assets path is fine).
3. On **Set up your application**, use:

| Field | Value |
|---|---|
| Project name | `mmhq` |
| Build command | `bash scripts/prepare-site-media.sh` |
| Deploy command | `npx wrangler deploy` |
| Preview command | leave default / blank |

4. Click **Deploy**.

Repo root has [`wrangler.toml`](../wrangler.toml) with `assets.directory = "./site"`. The build step copies Screenshots + brand PNGs into `site/media/` (gitignored); Wrangler then uploads `site/`.

5. After deploy succeeds: project → **Custom domains** → `mmhq.digitalexpl0it.com` (zone already on Cloudflare nameservers).

## Affiliate links

Product cards load from [`hardware.json`](hardware.json).

- **Amazon Associates** tag `metismeshcore-20` for most Nodes / Antennas / Cables.
- **Seeed Studio** SenseCAP affiliate code `b7Z7Ozr` (`?sensecap_affiliate=b7Z7Ozr&referring_service=link`).
- Each item uses a `links` array: `{ "merchant": "seeed"|"amazon", "url": "...", "coupon": "optional" }`. Multiple merchants render as separate Buy buttons on the card.

Each item has a `category` (Nodes / Antennas / Cables / Repeaters / Books) and a local thumbnail under [`thumbs/`](thumbs/). Disclosure copy is in the Hardware section and footer.

## Downloads

Primary buttons resolve the latest GitHub Release assets via the public API (Windows `.zip`, Linux `.AppImage`). If the API is unreachable, buttons fall back to the Releases page.
