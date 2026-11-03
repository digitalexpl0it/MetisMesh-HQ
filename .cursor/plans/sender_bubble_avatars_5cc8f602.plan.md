---
name: Sender bubble avatars
overview: Add per-contact letter avatars and matching soft tinted bubbles for incoming channel and DM messages, reusing the existing ConversationAvatar hash palette so colors stay stable.
todos:
  - id: tone-export
    content: Export conversation_avatar_tone; add message-bubble-tone-* (+ optional avatar-sm) CSS
    status: completed
  - id: channel-bubbles
    content: "Channel incoming: avatar + tinted bubble; keep emergency/outgoing as specified"
    status: completed
  - id: dm-bubbles
    content: "DM incoming: avatar + tinted bubble; outgoing unchanged"
    status: completed
  - id: verify-bubbles
    content: cargo check clean
    status: completed
isProject: false
---

# Per-contact avatars and tinted incoming bubbles

## Defaults

- Colors are **hashed** from contact identity (same FNV-style tone as the Messages rail), not random per paint.
- Seed: `sender_pubkey` when present, else display `device_name` (channel) / peer name (DM).
- Apply to **incoming** channel and DM rows. Outgoing stays the current sky bubble; no outgoing avatar.
- Emergency channel: keep signal border treatment; still show avatar; do not override emergency with a rainbow fill.

## 1. Shared tone helper

In [`src/ui/widgets.rs`](src/ui/widgets.rs):
- Make `conversation_avatar_tone` (and palette length if needed) `pub` so the Messages page can pick `message-bubble-tone-{n}` from the same seed as `ConversationAvatar`.

## 2. CSS palette for bubbles

In [`assets/form.css`](assets/form.css), add soft fills aligned with `.messages-avatar-0`…`7`:

- `.message-bubble-tone-0` … `.message-bubble-tone-7` — low-opacity background + subtle matching border
- Keep text on the normal foreground (readable on dark UI)
- Optional compact avatar size for chat rows if the rail size feels large beside bubbles (e.g. `.messages-avatar-sm` at ~1.35rem) — use that beside bubbles only

## 3. Channel thread UI

In [`src/ui/mod.rs`](src/ui/mod.rs) channel message loop (~4405):

- For each incoming message, compute `tone = conversation_avatar_tone(&seed)` and `seed` as above.
- Row layout: horizontal flex — `ConversationAvatar` (label = `device_name`, seed) to the left of the bubble column.
- Incoming (non-emergency) bubble class: base bubble + `message-bubble-tone-{tone}` instead of plain `bg-muted`.
- Incoming emergency: keep `border-signal/30 bg-background/80`; still show avatar.
- Replace the small Radio icon in the name chip with nothing extra (avatar already identifies the contact); keep the name text + time chip.
- Outgoing unchanged (sky bubble, right-aligned, no avatar).

## 4. DM thread UI

Same pattern for the DM scroller (~3956):

- Incoming: avatar left + `message-bubble-tone-{tone}` (seed = peer pubkey / title).
- Outgoing: sky, no avatar.

## 5. Verify

- `cargo check` clean (no warnings).
- Spot-check: two different senders get different tones; same sender stays consistent across reload; hover action overlay still clears the bubble corner.
