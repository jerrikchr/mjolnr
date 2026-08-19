# Persistence contract

What smed relies on from SQLite and `tokio-rusqlite`, read from official sources during Phase 4 and recorded here so a later change can be checked against the contract rather than against memory.

Sources consulted:

- <https://www.sqlite.org/wal.html>
- <https://www.sqlite.org/pragma.html>
- <https://docs.rs/tokio-rusqlite/0.7.0/tokio_rusqlite/>
- `tokio-rusqlite` 0.7.0 source (`src/lib.rs`), for the queue claim in §1.3

Verified: 2026-07-15, against `tokio-rusqlite` 0.7.0 → `rusqlite` 0.37.0 → `libsqlite3-sys` 0.35.0.

---

## 0. Why this document exists

Three facts below are counter-intuitive enough that getting them wrong produces a database that looks correct in tests and lies in production:

1. `PRAGMA foreign_keys` is **per-connection** and defaults **off**. A schema full of `REFERENCES` clauses enforces nothing unless every connection turns it on.
2. `tokio-rusqlite`'s internal queue is **unbounded**. It cannot be smed's backpressure boundary.
3. WAL mode is **persistent in the file**, so it is set once — but `busy_timeout` and `foreign_keys` are not, so they are set on every connection.

---

## 1. `tokio-rusqlite` 0.7

### 1.1 API actually used

```rust
pub async fn open<P: AsRef<Path>>(path: P) -> std::result::Result<Self, rusqlite::Error>
pub async fn open_in_memory() -> std::result::Result<Self, rusqlite::Error>

pub async fn call<F, R, E>(&self, function: F) -> std::result::Result<R, Error<E>>
where
    F: FnOnce(&mut rusqlite::Connection) -> std::result::Result<R, E> + 'static + Send,
    R: Send + 'static,
    E: Send + 'static;

pub async fn close(self) -> Result<()>
```

`Error<E = rusqlite::Error>` has exactly three variants:

| Variant | Meaning |
|---|---|
| `ConnectionClosed` | The background thread is gone. Every later `call` returns this. |
| `Close((Connection, rusqlite::Error))` | `close` failed; the connection is handed back for a retry. |
| `Error(E)` | The closure returned `Err`. |

### 1.2 Threading

> "A thread is spawned for each opened connection handle. When `call` method is called: provided function is boxed, sent to the thread through mpsc channel and executed."

This is the whole reason smed can satisfy "no blocking SQLite work in an async task" (`AGENTS.md` §4) without `spawn_blocking` at each call site: every closure body runs on that dedicated thread, never on a Tokio worker.

### 1.3 The queue is unbounded — this is load-bearing

`tokio-rusqlite` builds its message queue with `crossbeam_channel::unbounded::<Message>()` (`src/lib.rs:376` and `:387`). Nothing about `Connection::call` applies backpressure: a caller that outruns the SQLite thread grows the heap silently.

`AGENTS.md` §4 bans unbounded channels precisely because of this shape, so **`Connection` is never smed's ordering or backpressure boundary**. smed puts its own bounded actor in front (`src/store/sqlite/actor.rs`), and the actor awaits each `call` before accepting the next request. At most one closure is ever in the crate's queue, so its unboundedness is unreachable rather than merely unused.

### 1.4 `rusqlite` comes from the re-export

`tokio-rusqlite` re-exports its own `rusqlite` (`pub use rusqlite::{self, *};`) and pins `^0.37`. Adding a separate `rusqlite = "0.40"` would resolve a **second** `libsqlite3-sys`, i.e. two SQLite libraries in one binary. The rule is therefore to use its compatible re-export and never add a mismatched `rusqlite`. `cargo tree -i rusqlite` must show exactly one version, reached only through `tokio-rusqlite`.

### 1.5 `bundled`

smed enables `tokio-rusqlite/bundled`, which compiles SQLite from source into the binary rather than linking the host's `libsqlite3`.

- The WAL, `busy_timeout`, and `integrity_check` behaviour smed tests against is the behaviour that ships, not whatever the OS supplies.
- It matches the static-binary posture of `docs/adr/0001-all-rust-ratatui.md`, the same reason `reqwest` uses `rustls` rather than system OpenSSL.
- Cost: a C compiler at build time, and a slower cold build.

---

## 2. SQLite settings

### 2.1 Per-connection, set on every open

| Pragma | Value | Why |
|---|---|---|
| `foreign_keys` | `ON` | **Defaults to off** (since 3.6.19) and is per-connection. Without it, every `REFERENCES` in §3 is decoration. It is also a no-op inside a transaction, so it is set immediately after opening, before any `BEGIN`. |
| `busy_timeout` | `5000` (ms) | WAL still returns `SQLITE_BUSY` when a writer collides. A finite timeout turns that into a bounded wait; no timeout turns it into an instant failure under trivial contention. Finite rather than infinite, so a stuck writer surfaces as an error instead of a hang. |

### 2.2 Persistent in the file, set once at migration

| Pragma | Value | Why |
|---|---|---|
| `journal_mode` | `WAL` | "The WAL journaling mode is persistent; after being set it stays in effect across multiple database connections and after closing and reopening the database." Readers do not block the writer, which is what lets a second smed process read a session that another owns. |
| `user_version` | `1` | The schema version. SQLite never touches it, so it is the migration ledger (§4). |

### 2.3 Concurrency

- Readers and the writer run concurrently; **exactly one writer at a time** per database.
- WAL requires all processes on one host and **does not work on network filesystems** — it needs shared memory for the wal-index.
- Auto-checkpoint runs at roughly 1000 pages and when the last connection closes. smed does not tune this; nothing here is throughput-bound.

SQLite's single-writer rule covers the *database*. It says nothing about two smed processes driving the *same session*, which is a different problem — see §5.

### 2.4 `integrity_check`

`PRAGMA integrity_check` is O(N log N) over the database. Running it at startup is forbidden, so it is only reachable through `smed diagnostics --integrity`. `quick_check` is O(N) but skips index and UNIQUE consistency, which is most of what a corruption check is for; smed does not offer the weaker check as if it were the real one.

---

## 3. Schema

The durable schema is summarized here; `src/store/sqlite/schema.rs` is the
executable source of truth.

Tables: `projects`, `sessions`, `events`, `checkpoints`, `provider_profiles`, plus `session_owners` (§5).

Additions beyond the base schema, each for a stated reason:

| Addition | Reason |
|---|---|
| `session_owners` table | A session must have exactly one writer: no split-brain writer is permitted. Nothing in the §9 schema can express ownership. |
| `events.event_id TEXT NOT NULL UNIQUE` | Already in §9. Named here because the UNIQUE constraint *is* the duplicate-event-ID guard; it is not incidental. |
| `PRIMARY KEY (session_id, sequence)` | Already in §9. It is the gap/duplicate-sequence guard, enforced by the database rather than by smed remembering to check. |

`sessions.status` values: `active`, `ended`. `provider_profiles` is created by the migration but remains empty in Phase 4; provider-profile writes arrive with the multi-provider phases.

### 3.1 What is never stored

Enforced by `tests/persistence_secrets.rs`, which scans every byte of a populated database file:

- No API keys, `Authorization` headers, or any credential.
- No environment snapshots.
- No exact-command approval grants (§6).
- No `TextDelta` rows ("coalesce text deltas; do not commit one row per token").

---

## 4. Migrations

Versioned and transactional, no framework:

1. Read `PRAGMA user_version`.
2. If it is **greater** than `SCHEMA_VERSION`, refuse with `StoreError::UnsupportedSchema` **before changing journal mode or any other persistent setting**. A newer smed may have written columns this build would silently ignore, and a store that half-understands its own data is worse than one that stops.
3. Apply each migration whose version exceeds the current one, each inside one transaction that also bumps `user_version`. A failed migration rolls back whole; there is no half-migrated state to diagnose.

Idempotence follows from `user_version` rather than from `IF NOT EXISTS`: re-opening an up-to-date database applies nothing.

`journal_mode = WAL` is issued outside the transaction — it is persistent, and SQLite will not change journal mode inside one.

---

## 5. Session ownership

SQLite serialises writers to the *file*. It has no opinion about two smed processes appending to the same *session*, which would interleave two runs into one transcript — a split-brain writer with a perfectly consistent database.

smed therefore leases a session:

- `session_owners(session_id PRIMARY KEY, owner_token, acquired_at)`.
- Acquiring is one SQLite transaction whose `INSERT ... SELECT` succeeds only while the session row is active. The status gate and lease insert therefore cannot race across processes; the primary key makes a second writer conflict.
- A conflict refuses the open plainly and names the holder. Phase 4 has no read-only session client.
- A clean shutdown deletes the row.
- An ended session cannot acquire a new lease and cannot be resumed for work.

**A crash leaves the row behind, and smed does not steal it.** It cannot prove the holder is dead — the honest position under `AGENTS.md` §1.2 (fail closed) and §1.3 (never lie about state). `smed sessions release <id>` is the explicit human act that reclaims it. This is not an accident of the design: a crashed session is exactly the one whose interrupted work needs a human anyway (§6).

---

## 6. What survives, and what must not

Restored on resume — a validated checkpoint plus every durable event after it:

session identity · project root · canonical messages · active provider and model · usage and budgets · quota-reserve boundary · latest structured handoff · policy · read set · tool results · last mutation sequence · successful command evidence · activated skill names · project-skill workspace trust.

Compact resume is a provider-request projection, never a rewrite. SQLite keeps the complete canonical message history; the provider receives the latest handoff, bounded recent messages, and messages added after the compact open. `HandoffCreated` and `QuotaBoundaryReached` are durable audit events, while raw `QuotaReported` snapshots remain display-only until a threshold becomes governing state.

The store validates the whole extent before projection: checkpoint identity must match its row, its covered count cannot exceed either the durable event count or `sessions.last_sequence`, and the event suffix must reach that terminal count without a gap. A checkpoint also carries session status, so one covering `SessionEnded` cannot resurrect an ended session when there is no later event to replay.

The stored project path is an identity, not a hint. Resume requires the checkpoint and project row to agree, re-canonicalises that path, and refuses if it now resolves elsewhere. `run_command` repeats the same identity check immediately before process spawn to close the resume-to-effect symlink window.

Project identities are stored as SQLite `TEXT`. A canonical workspace path containing non-UTF-8 bytes is explicitly refused; smed never stores a lossy replacement that would later fail or rebind its identity check. Project canonicalisation failures and failed validation tasks are surfaced in the runtime snapshot rather than silently leaving the previous root in place.

Policy selected before a session is created is appended as `PolicyChanged` immediately after `SessionCreated`. The live session therefore begins with the same policy that checkpoint-plus-tail replay reconstructs, including a crash before the first checkpoint.

**Never restored: exact-command approval grants.** `ApproveExactForSession` is scoped to one session, and a grant surviving a restart would silently widen it. This is a property of the type, not a rule someone must remember: `SessionCheckpoint` has no field for it, and `ApprovalResolved` events are replayed for their ordering and their audit value only — the projection never rebuilds `exact_commands` from them. A future field would have to be added deliberately, in a diff a reviewer can see.

Skill activation follows the opposite rule because it is canonical model context, not side-effect authority. A successful `activate_skill` result carries a durable `SkillActivated { name, project }` effect. Replay derives the activated-name set and workspace trust from those effects, while the checkpoint caches both fields. The effect grants no tool permission; scripts still cross `run_command` policy after resume.

Model selection is also event-sourced. `ModelChanged` is durable before live provider/model state changes, and replay restores it. A switch blocked by active work or incompatible capabilities records `ModelChangeRefused` with `RUN_ACTIVE` or `PROVIDER_INCOMPATIBLE_MODEL`; it never changes the session row's active model. Payload wire version 3 introduces that refusal event while continuing to read older payload versions.

---

## 7. Recovery

The database is reconstructable from the latest valid checkpoint plus every later durable event. Checkpoints are an optimisation and a safety net, never the only truth: a mutation completed after the last checkpoint is recovered from its `ToolCompleted` event.

Interrupted work is typed, never a boolean (`src/core/recovery.rs`):

| State | Evidence | Meaning |
|---|---|---|
| `ProposalUnapproved` | `ToolProposed`, no `ApprovalResolved`, no outcome | The side effect never started. Safe, and still requires acknowledgement. |
| `EffectUncertain` | A policy-authorised proposal or approving `ApprovalResolved`, with no `ToolCompleted`/`ToolFailed` | **The effect may or may not have happened.** smed cannot tell and will not guess. Authority is retained for explanation, but certainty is the state axis. |
| `ProviderTurnInterrupted` | `RunStarted`, no terminal run event | A provider call died mid-stream. Never replayed: it may have produced tokens (`AGENTS.md` §4). |

Resume never executes an interrupted tool and never replays an interrupted provider call. Autonomous work is blocked with `RECOVERY_REQUIRES_DECISION` until a human resolves it, and the resolution is itself durable.

A user message submitted while recovery or a durability failure blocks the session is refused before a run exists. It creates no `MessageAppended`, `RunStarted`, or orphan `RunFailed` event; the runtime snapshot carries the blocking state the client must render.

`ToolCompleted` and `ToolFailed` are the canonical durable source for tool-result messages. Projection reconstructs the model-facing result directly from that terminal event; there is no second `MessageAppended` transaction whose loss could orphan a completed effect.

Shutdown checkpoints only settled, fully durable state. If a provider/tool run is active, recovery is unresolved, or an append has failed, smed cancels live work, flushes the accepted event tail, and releases the lease **without** writing a checkpoint over it. The next resume therefore sees the interruption instead of a clean snapshot that accidentally erased it. A checkpoint failure on the settled path is returned by `Runtime::close`; it is never reported as a successful save.

The asymmetry in the table is the point. 's anti-pattern — "do not infer that an interrupted command failed merely because no completion event exists" — is why `EffectUncertain` exists as its own state instead of being folded into a failure.
