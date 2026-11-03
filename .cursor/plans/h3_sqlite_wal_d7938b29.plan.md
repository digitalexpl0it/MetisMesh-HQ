---
name: H3 SQLite WAL
overview: "Harden SQLite WAL setup: verify journal mode on open, set durable pragmas for multi-radio ingest, add a concurrency smoke test, and document metis.db-wal / metis.db-shm for operators who copy files by hand."
todos:
  - id: pragmas
    content: Add apply_runtime_pragmas (WAL assert, busy_timeout, synchronous=NORMAL) on open + restore
    status: completed
  - id: tests
    content: "Unit tests: journal_mode=wal on file open; concurrent short writes on Arc<Db>"
    status: completed
  - id: docs
    content: Document metis.db-wal/-shm in README + OFFLINE_RUNBOOK; tick H3 + CHANGELOG
    status: completed
isProject: false
---

# H3 — SQLite WAL durability

## Context

[`Db`](src/db.rs) is already a single `Mutex<Connection>` shared via `Arc` across radio tasks, poller, and UI. `journal_mode=WAL` is set in `Db::open` and again after restore, but nothing asserts it stuck, `from_conn` (tests / memory) never sets WAL, and there is no `busy_timeout` / checkpoint policy. Encrypted `.metisbak` already uses the online Backup API (consistent snapshot). Raw disk copies of `metis.db` alone can miss `-wal`/`-shm` — that is undocumented.

Mutex serialization already prevents classic multi-connection busy storms inside the process. H3 focuses on confirming WAL, making open/restore resilient, a small concurrent-write smoke test, and operator docs — not a connection pool rewrite.

## Implementation

### 1. Centralize open pragmas + assert WAL

In [`src/db.rs`](src/db.rs), add `apply_runtime_pragmas(conn)` called from `Db::open` and after restore re-WAL:

- Set `journal_mode=WAL`, then read it back and error if the result is not `wal` (some network/FS setups reject WAL)
- Set `busy_timeout=5000` — covers the Backup API second connection and any future readers
- Set `synchronous=NORMAL` — standard durable-enough WAL pairing
- Leave `wal_autocheckpoint` at SQLite default (~1000 pages); no custom checkpoint daemon unless unbounded `-wal` growth shows up later

### 2. Tests (success check in todo.md)

Add focused unit tests in `src/db.rs`:

- `open_sets_wal_journal_mode`: temp-dir file → `Db::open` → `PRAGMA journal_mode` is `wal`; after a write the `-wal` sidecar may exist
- `concurrent_writes_survive`: one `Arc<Db>`, several threads doing short inserts (audit/settings) with no panic and no SQLITE_BUSY; join cleanly

Mark the success-check line in [`todo.md`](todo.md) once these pass.

### 3. Docs — `-wal` / `-shm` beside the main file

Short operator-facing notes:

- [`README.md`](README.md) data-folder bullet: live store is `metis.db` plus optional `metis.db-wal` and `metis.db-shm`; prefer Settings Backup for copies; if copying the folder cold, include all three or stop HQ first so WAL can checkpoint
- [`docs/OFFLINE_RUNBOOK.md`](docs/OFFLINE_RUNBOOK.md): same one-liner near the data-folder help
- Tick **H3** in `todo.md` and a brief [`CHANGELOG.md`](CHANGELOG.md) Unreleased line

## Out of scope

- H2 OS smoke matrix
- Multi-connection pool / reader threads
- Changing encrypted backup format
- Aggressive checkpoint timers or vacuum on shutdown

## Verify

- `cargo test --features desktop` filtered to the new db tests
- Manual: launch once, confirm sidecars may appear under the data dir after activity