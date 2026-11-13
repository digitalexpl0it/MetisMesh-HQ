# Live RX check — field matrix

> **Canonical copy for operators:** open **Help → Live RX check** inside Metis Mesh HQ.
> This file mirrors that in-app topic for GitHub / print-from-source.

Confirm incoming mesh text appears in HQ within about **two seconds without restarting**.

## Pass criteria

1. Companion shows Connected / Companion ready.
2. Channel and DM traffic from another device lands in Messages within ~2s while HQ stays open.
3. Compose and Device CLI stay usable while messages arrive (send while receiving).

## Matrix

| Transport | Idle channel | Idle DM | Send while RX |
|-----------|:------------:|:-------:|:-------------:|
| USB       | ☐            | ☐       | ☐             |
| Bluetooth | ☐            | ☐       | ☐             |
| TCP       | ☐            | ☐       | ☐             |

Check each cell you can exercise on the desk radio. You do not need every transport on every machine — cover what you ship.

## Soak (quick)

1. Leave HQ connected on one transport above.
2. From a phone / T-Deck: send ~10 short channel messages, then 2 DMs to this HQ.
3. While those arrive, send one channel message from HQ Compose and one short CLI (or Device) command.
4. All RX should show in Messages without restart; HQ send/CLI should complete.

## Optional logs

```bash
# Linux AppImage example
./metis-mesh-hq-x86_64-vVERSION.AppImage --dev 2>&1 | tee /tmp/metismesh-rx.log
```

Look for `sync ready`, `MessagesWaiting`, `msg drain cleared` / `drain deferred`, and `radio lock: … wait Nms` (operator lock stalls). Soft status may show “Receiving… radio busy, retrying.” — Compose stays open.

## Related

- GitHub issue tracking live RX stuck until restart: https://github.com/digitalexpl0it/MetisMesh-HQ/issues/2
- Offline outage steps: [OFFLINE_RUNBOOK.md](OFFLINE_RUNBOOK.md) / Help → Offline runbook
