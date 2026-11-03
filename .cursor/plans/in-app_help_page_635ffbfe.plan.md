---
name: In-app Help page
overview: "Add a top-level Help nav page with a sticky topic TOC: operator desk guide covering the whole HQ, plus a shareable “For radio users” section on signup and private channels."
todos:
  - id: help-nav
    content: Add Page::Help, Help icon, nav button, search topics, About Open Help link
    status: completed
  - id: help-page
    content: Create help_page.rs + form.css layout with TOC and operator+radio-user sections
    status: completed
  - id: help-docs
    content: Update todo.md, CHANGELOG, README feature blurb
    status: completed
isProject: false
---

# In-app Help page

## Placement

- New `Page::Help` in [`src/ui/mod.rs`](src/ui/mod.rs).
- Sidebar: **above Settings** (after CLI), collapsed + expanded; icon `IconName::Inbox` or a small new `Help`/`Book` circle-`i` icon in [`src/ui/icons.rs`](src/ui/icons.rs) — prefer a dedicated **Help** (circle-i) icon so it does not collide with Inbox.
- Wire `NAV_SEARCH_INDEX` topics: help, signup, enroll, channel, secret, weather, flasher, backup, revoke, METIS-INFO, METIS-NET, etc.
- Match arm: `Page::Help => rsx! { HelpPage {} }`.
- About card keeps version/license; add one Ghost button **Open Help** that sets `page` to Help (pass via existing pattern or a small signal — simplest: store a one-shot `open_help` like `open_about_after_welcome`, or navigate from a callback if Help is only opened from nav; for About link, reuse the same `page` signal already in `App`).

## Structure

New module [`src/ui/help_page.rs`](src/ui/help_page.rs) (keep `mod.rs` smaller), registered in [`src/ui/mod.rs`](src/ui/mod.rs) like `cli_page` / `weather_page`.

Layout (CSS in [`assets/form.css`](assets/form.css) — frozen Tailwind lacks reliable layout utilities):

- `.help-page` two-column on wide: left sticky **Topics** list, right scrollable articles.
- Narrow: topics as horizontal wrap pills above content.
- Selecting a topic scrolls/shows that section (signal `help_topic`; sections are always in DOM with `id` anchors — click sets topic and uses a short in-page scroll via focusing the section or CSS `:target` / simple show-all with scroll-into-view via `document::eval` only if needed; default: **all sections visible**, TOC jumps with `scroll_to` eval on section id — matches offline desk reading).

## Content (operator + radio user)

Concise cards / `h3` sections — no secrets, no FEMA/NWS cosplay language. Cover:

1. **What HQ is** — local app owns radios; no cloud login.
2. **For radio users (shareable)** — DM companion `signup` / `signup Name`; wait for approve; add **METIS-INFO** and **METIS-NET** from the two DMs (secrets never shown in HQ message list as raw hex — tell them to read the DM on their phone); `status` checks enrollment; bot `#weather` etc. only after enrolled on INFO.
3. **Connect the companion** — Settings → Connect; BLE (phone must disconnect), USB, TCP `:5000`; primary radio owns bot + enrollment TX.
4. **Enrollment desk** — Inbox Approve/Deny; Roster Revoke rotates both secrets and re-sends to remaining enrolled.
5. **Channels** — INFO = bot/alerts; NET = peer traffic; extra privates via Roster Add channel.
6. **Messages / Inbox** — DMs vs community channels; Inbox = signups + held alerts.
7. **Map / Tracker / Infrastructure** — GPS self-reported; Tracker whitelist; claim repeaters/rooms via companion CLI (not pool companions).
8. **Weather and alerts** — METIS-INFO bot; Settings services; auto vs Inbox confirm.
9. **CLI / Flasher** — Device CLI roles; offline locker Sync → Ready → Export/Import `.zip`; flash checksum guard.
10. **Backup** — encrypted `.metisbak`; note `metis.db` + `-wal`/`-shm` if copying folder cold.
11. **First hour offline** — short pointer that mirrors [`docs/OFFLINE_RUNBOOK.md`](docs/OFFLINE_RUNBOOK.md); About keeps the one-page checklist (no duplication of the whole runbook body).

## Docs / todo

- Tick a small **Help** item under Phase J or a new note in [`todo.md`](todo.md).
- One [`CHANGELOG.md`](CHANGELOG.md) Unreleased line.
- README one-liner under features if there is already a UI bullet list.

## Verify

- `cargo check --features desktop`
- Nav: Help appears above Settings; search finds “signup” / “flasher”.
- Help sections readable; About → Open Help works.