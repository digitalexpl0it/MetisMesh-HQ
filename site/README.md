# Metis Mesh HQ — marketing site

Static dark landing page for Cloudflare Pages (`mmhq.digitalexpl0it.com`).

## Local preview

From the repo root:

```bash
bash scripts/prepare-site-media.sh
cd site && python3 -m http.server 8787
```

Open `http://127.0.0.1:8787`.

## Cloudflare Pages

1. Cloudflare dashboard → **Workers & Pages** → **Create** → **Pages** → connect the `MetisMesh-HQ` GitHub repo.
2. Build settings:
   - **Build command:** `bash scripts/prepare-site-media.sh`
   - **Build output directory:** `site`
   - Root / working directory: repository root (default)
3. Deploy on `main` (or your chosen production branch).
4. **Custom domains** → add `mmhq.digitalexpl0it.com`.
   - With Cloudflare already as the domain’s nameservers, Pages will create the DNS record for you.
   - If the zone is on Cloudflare but you manage records yourself, add a CNAME from `mmhq` to the Pages `*.pages.dev` hostname shown in the project.

## Amazon Associates

Product cards load from [`hardware.json`](hardware.json) (Associates tag `metismeshcore-20`). Each item has a `category` (Nodes / Antennas / Cables / Repeaters) and a local thumbnail under [`thumbs/`](thumbs/). Disclosure copy is in the page footer.

## Downloads

Primary buttons resolve the latest GitHub Release assets via the public API (Windows `.zip`, Linux `.AppImage`). If the API is unreachable, buttons fall back to the Releases page.
