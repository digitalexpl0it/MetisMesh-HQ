# Guide pack format (HQ-native)

Metis Mesh HQ installs offline reference packs under `data/guides/packs/<id>/`.
Packs are **not** merged into `metis.db`. Kiwix `.zim` is reserved for a later adapter.

## Zip layout

A downloadable pack is a `.zip` whose root (or single top-level folder) contains:

```
manifest.json
articles/
  example.md
media/          # optional images referenced from Markdown
some.pdf        # optional; listed under attachments, not full-text indexed
```

## Remote files (no HQ hosting)

Catalog rows may list upstream HTTPS files. On **Install**, HQ downloads each file, verifies `sha256`, and synthesizes `manifest.json` plus `articles/about.md` locally. Binaries are **not** committed to the Metis Mesh HQ repo.

```json
{
  "id": "fema-are-you-ready",
  "name": "FEMA Are You Ready?",
  "version": "2020-09",
  "source": "Ready.gov / FEMA",
  "license": "US Government work (public domain)",
  "description": "…",
  "format": "hq-native",
  "files": [
    {
      "id": "are-you-ready",
      "title": "Are You Ready? Guide (PDF)",
      "url": "https://www.ready.gov/sites/default/files/2021-11/are-you-ready-guide.pdf",
      "sha256": "…",
      "size_bytes": 4233522,
      "path": "attachments/are-you-ready-guide.pdf"
    }
  ]
}
```

| Path extension | Treated as |
|---|---|
| `.pdf` (default) | Attachment (system viewer) |
| `.md` / `.txt` | Searchable article |
| `.html` / `.htm` | Searchable article (sanitized HTML; relative `img` files are fetched beside the page on install). Prefer HTML-with-images over plain `.txt` when upstream offers it. |

**Export:** every article can Export MD / Export PDF. HTML packs derive Markdown (and PDF text) from the sanitized content so illustrations become alt-text captions in the export.

Refresh hashes without committing downloads:

```bash
./scripts/hash-guide-urls.sh
./scripts/hash-guide-urls.sh 'https://example.org/guide.pdf'
```

## `manifest.json`

```json
{
  "id": "my-pack",
  "name": "My Pack",
  "version": "1.0.0",
  "source": "Upstream project name",
  "license": "CC-BY-4.0",
  "description": "Short blurb for the Marketplace.",
  "categories": ["water", "preparedness"],
  "articles": [
    {
      "id": "example",
      "title": "Example article",
      "path": "articles/example.md",
      "tags": ["water", "first-aid"],
      "summary": "One-line summary for search snippets."
    }
  ],
  "attachments": [
    {
      "id": "poster",
      "title": "Wall poster (PDF)",
      "path": "poster.pdf"
    }
  ]
}
```

- Prefer **Markdown** (`.md`) for articles. HTML is accepted but sanitized; remote scripts and `http(s)` images are blocked in the reader.
- Image paths in Markdown must be **relative** to the pack root (e.g. `media/boil.png`).
- Attachment PDFs are opened with the system viewer; only title/path are indexed for search.

## Categories

Fixed vocabulary (catalog + Guides/Marketplace dropdown filters). Packs may list **one or more** ids:

| id | Label |
|----|--------|
| `water` | Water |
| `medical` | Medical |
| `radio` | Radio |
| `shelter` | Shelter |
| `hazards` | Hazards |
| `knots` | Knots |
| `preparedness` | Preparedness |
| `fieldcraft` | Field craft |

Unknown ids are ignored when normalizing. Free-form `tags` remain for FTS keywords; `categories` drive the UI filters.

## Catalog entry

Marketplace rows live in `assets/guides/catalog.seed.json` (bundled) and may be refreshed from `remote_url` when online. Each pack may set:

| Field | Meaning |
|---|---|
| `bundled` | Key for an embedded sample (e.g. `hq-basics`) — install copies from the binary |
| `download_url` | HTTPS zip URL (pre-built HQ-native zip + optional top-level `sha256` / `size_bytes`) |
| `files` | Remote PDFs/text — downloaded and synthesized into a local pack |
| `categories` | List of category ids from the vocabulary above |
| `sha256` | Expected hex digest for a zip `download_url` |
| `size_bytes` | Optional exact size check for a zip |
| `format` | `hq-native` (v1) or `zim` (stub — not installable yet) |

## On disk after install

```
data/guides/catalog.json
data/guides/packs/<id>/…
data/guides/index.db      # FTS5 index only for guides
data/guides/downloads/    # temporary zips
```
