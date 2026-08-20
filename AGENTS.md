# AGENTS.md — engineering standards for mjolnr

Read this before writing code. It is the operational contract for every contributor, human or agent.

- **What done means** → [`docs/definition-of-done.md`](./docs/definition-of-done.md) (the finish line and what it excludes)
- **Where the product surface is going** → [`docs/ux-workspace-direction.md`](./docs/ux-workspace-direction.md) and [`docs/adr/0003-shared-rust-core-tui-tauri-clients.md`](./docs/adr/0003-shared-rust-core-tui-tauri-clients.md)
- **How to build it** → this file

Where this file and an accepted ADR disagree, **the ADR wins on the decision and the stricter rule wins on quality**. Report the conflict rather than resolving it silently.

## 0. What mjolnr is, in one paragraph

mjolnr is a local-first AI coding harness with a Rust governance core, a
terminal-launched Ratatui client, and a Tauri desktop client under active
development. Both clients consume the same runtime truth and neither owns an
agent loop. mjolnr owns its native tools, policy gates, sessions, and skills, and
talks directly to model provider APIs (OpenAI, Anthropic, Gemini, OpenRouter,
Ollama). External agent runtimes may join the workspace through explicit
compatibility boundaries, but their work keeps its external provenance until a
mjolnr-owned adapter proves stronger guarantees. The central promise is
**governed execution**: the model proposes, mjolnr's deterministic code disposes.
Code that weakens a guard is not a shortcut — it is a defect in the thing being
sold.

## 1. Prime directives

These override convenience, elegance, and velocity. In a conflict, they win.

1. **The model proposes; code disposes.** Every side effect passes a deterministic gate mjolnr owns. No guard may be conditioned on model output claiming it is safe.
2. **Fail closed.** Unknown tool tier → `Execute`. Unknown path → rejected. Missing capability → refuse before the request. Ambiguity resolves toward refusal, never toward action.
3. **Never lie about state.** Reported success requires evidence. `verified` requires a successful post-mutation command. If a thing was not checked, say it was not checked — in the UI, in reports, in the README.
4. **Uncertain side effects are never retried automatically.** If mjolnr cannot prove a write or command did not happen, it asks a human. Losing work beats duplicating it.
5. **Secrets never leave their boundary.** Not into logs, argv, SQLite, `Debug` output, panics, fixtures, child environments, or the screen.

## 2. Architecture — how we avoid spaghetti

### 2.1 Dependency direction is law

```text
clients (Ratatui + Tauri) → runtime → core (traits + types)
                                       ↑        ↑
                                  providers   tools / store / policy / context
```

- **`core` depends on nothing internal.** It defines traits and types only.
- **`tui` is a client.** It renders snapshots and emits commands. It must never import `providers`, `tools`, `store`, or `policy`, and never execute a tool or call a provider.
- **The Tauri application is also a client.** Its frontend may own
  presentation state, but authoritative plan, approval, recovery, execution,
  and verification state remains in Rust runtime/store contracts. It must not
  recreate the agent loop or policy gate in TypeScript.
- **`providers` know wire formats and nothing else.** No policy, no persistence, no UI.
- **`policy` knows rules, not callers.** It cannot ask which provider or model triggered a decision.
- **No non-client module may import a client. Ever.**

Enforced by `tests/architecture.rs` (Phase 1), which scans `src/` for forbidden import edges and fails the build. If you need an exception, you need an ADR, not a `#[allow]`.

### 2.2 Single crate now, extractable always

One Cargo package is a deliberate decision. It is **not** licence to build a monolith.

> **The extraction test:** could this module be lifted into its own crate by moving the directory and adding a `Cargo.toml`? If not, its boundaries are already wrong.

Apply this test whenever a module grows. Failing it is the earliest signal of rot — earlier than any line count.

### 2.3 God-module prevention

- **One module, one reason to change.** If you cannot name a module's single responsibility in one sentence without "and", split it.
- **Soft cap: 400 lines per file.** Crossing it is a review trigger, not an error. Crossing 600 requires a note in the commit explaining why splitting is worse.
- **Soft cap: 50 lines per function**, 3 levels of nesting. Extract named helpers instead of commenting sections of a long function. (The `too_many_lines` lint backstops this at 100 — the lint is the ceiling, 50 is the target. Do not read a passing build as approval of an 90-line function.)
- **No global mutable state.** No `static mut`, no `lazy_static` registry, no singletons. Dependencies are passed in, which is also what makes them testable.
- **State mutates in one place.** Runtime state changes via the command reducer; TUI state changes via the TUI reducer. No side-channel mutation.
- **The TUI never holds the authoritative transcript.** It holds a view. The runtime owns truth.

### 2.4 Types over conventions

- Make illegal states unrepresentable. Prefer an enum over a `bool` plus a comment; prefer a newtype (`SessionId`) over a bare `String`.
- A validated value gets a distinct type. Once a path is `ContainedPath`, it cannot be confused with a `PathBuf` from the model.
- `pub(crate)` by default. `pub` is a deliberate act, not a default.

## 3. Security

- **`unsafe` is forbidden** — `unsafe_code = "forbid"`. No exceptions in this codebase.
- **Secret types implement `Debug` manually and print `<redacted>`.** Never `#[derive(Debug)]` on anything holding a credential; a derived `Debug` plus one `tracing` call is a leak. Zeroize on drop.
- **Credentials come from mjolnr's owner-only credential files or the environment only.** No other file location, ever, including "just for development". This covers every secret class without exception: provider API keys, provider OAuth tokens, and MCP server bearer/OAuth tokens, local or remote. The OS keyring was deliberately abandoned — see the `src/store/secrets.rs` header for what it cost and what the trade gives up.
- **Never pass a secret as a CLI argument** — argv is world-readable and lands in shell history.
- **Scrub the child environment** before `run_command`. Provider keys must not be inheritable.
- **Recheck path containment immediately before every filesystem side effect**, not only at validation time. The gap between check and use is the vulnerability.
- **Never build a shell string from model text.** Use an argument vector, or an explicit shell contract whose exact text is displayed at approval.
- **Validate tool arguments after the last transformation**, immediately before `execute`. A hook that mutates arguments post-validation defeats validation.
- **A model can never self-approve.** Approval is a human act or a pre-declared policy rule, never an inference.
- **Fixtures are redacted at capture time**, never "cleaned later".
- **Do not claim OS-level sandboxing unless real containment is implemented and verified.** The current policy gate is a policy gate. Say so in the UI and README.

## 4. Stability and concurrency

- **Bounded channels only.** `unbounded_channel` and `UnboundedSender`/`UnboundedReceiver` are denied via `clippy.toml` `disallowed-methods`/`disallowed-types`. Backpressure is a feature: a slow TUI must slow the producer, not grow the heap.
- **Cancellation is plumbed everywhere.** Every async operation that can outlive a keystroke takes a `CancellationToken`. Cancel must stop the work and emit exactly one terminal event.
- **Never hold a lock across `.await`.** Prefer message passing to shared locks.
- **No blocking I/O in async contexts.** Use `spawn_blocking` or the async API.
- **Terminal restoration is guaranteed by an RAII guard plus a panic hook** — not by a cleanup call at the end of `main`. It must survive normal return, error, Ctrl-C, and panic.
- **No `unwrap`/`expect`/`panic!`/indexing in non-test code** (clippy-denied). A panic in a TUI app corrupts the user's terminal. Return typed errors.
- **Never `print!`/`println!`/`dbg!`** — stdout is the alternate screen. Denied by clippy. Diagnostics go to the file tracing subscriber.
- **Persist intent before effect.** `ToolProposed` and `ApprovalResolved` are durable *before* the side effect starts, so recovery can reason about what might have happened.
- **No auto-retry after partial output.** A stream that produced tokens and then failed is not safe to replay.

## 5. Performance

This workload is I/O-bound: nearly all wall-clock is waiting on provider APIs. **Do not micro-optimize compute.** Optimize the three things that actually bite:

- **Bounded memory under stream load.** Coalesce text deltas; never one DB transaction per token; never one event row per token.
- **Redraw only when dirty**, or on a modest tick. Not per delta.
- **Never clone the whole transcript per frame.** Snapshots share (`Arc`) or diff. An O(n) copy per render is O(n²) over a session — the classic TUI death.
- Bound every output: tool results, diffs, command capture, search hits. Truncate with explicit metadata rather than silently.

Measure before optimizing anything else. A benchmark or a profile goes in the commit; "felt slow" does not justify a rewrite.

## 6. Errors and reason codes

- **Reason codes are a public contract**. Human-readable messages may change freely; codes may not. Tests assert on codes, never on prose.
- **Typed errors per module via `thiserror`.** No `anyhow` in library code — it erases the taxonomy the guards depend on. `anyhow` is acceptable in `main.rs` and tests.
- **Errors carry context, not blame.** Include what was attempted and why it was refused, so the model can correct and the user can understand.
- **A refusal is a normal result, not an exception.** Denied tools return a structured result to the model so the loop can continue.

## 7. Testing

- **Every guard has a test that proves it refuses.** A guard without a negative test is decorative.
- **The default test run touches no network and no real credentials.** Provider tests use local mocks (`wiremock`) and redacted fixtures. Live tests are `#[ignore]` and opt-in.
- **Core is testable without the TUI.** If a test of the agent loop needs a terminal, the boundary is broken.
- **Determinism.** No sleeps-as-synchronization, no wall-clock dependence, no test order coupling. Fake the provider; fake the clock where it matters.
- **Property-test the parsers.** Stream decoders must survive arbitrary chunk boundaries — that is where real provider bugs live.
- **Test the crash matrix, not just the happy path.** Interruption between propose/approve/execute/complete is the interesting case.
- Tests may `unwrap` freely (`clippy.toml` allows it in tests). Clarity beats ceremony there.

## 8. Dependencies and provenance

mjolnr is intended to become open source. Provenance must be reviewable from the first commit.

- **Do not copy, port, translate, or lightly refactor code from researched agent repositories** (Pi, Oh My Pi, OpenCode, OpenTUI, OpenGUI, Wayland, Codex, Claude Code, Hermes, and others). Not their code, tests, comments, naming, structure, or internal protocols. **Do not ask a model to disguise a port.** They inform requirements and known failure modes only.
- **Implement from official sources**: provider API docs, published standards, RFCs, and crate documentation.
- **New dependencies require justification** in the commit: purpose, licence, rejected alternatives, removal cost. Prefer the standard library when it stays clear and safe.
- **Every direct dependency is recorded in `THIRD_PARTY.md`** with purpose, version, licence, and source URL. `deny.toml` enforces licence, advisory, and source policy.
- **Lockfile is committed and pinned.**
- **No unofficial provider SDKs.** Adapters are written against documented REST contracts.
- If third-party code is ever deliberately incorporated, preserve its notice, comply with its licence, and record it in an ADR. Plainly.
- **Licence is Apache-2.0** (owner decision, 2026-07-31). `LICENSE` holds the canonical text verbatim — never edit it — and `NOTICE` holds the copyright line. Both manifests declare `license = "Apache-2.0"`, and `cargo deny check` now verifies our own package against the allow list rather than exempting it. A dependency under a copyleft licence would force a relicence, so the permissive-only rule above is stricter than it looks.
- **Publishing to crates.io is still unselected.** Keep `publish = false` and do not create a release on a registry without Jerrik's direction. This is now independent of the licence.
- **Publication is gated on [`docs/pre-release-checklist.md`](./docs/pre-release-checklist.md).** Read it before doing anything that makes the repository visible.

## 9. Git and review

- **Stop at every delivery checkpoint.** Do not start the next piece of work. Report using the template below.
- **One change, one commit.** No mega-commits spanning unrelated work. Stage deliberately; verify `git diff --cached --name-status` before committing.
- **Never proceed past a failed checkpoint by documenting the failure as acceptable.** A blocked phase is a stop, not a note.
- **Never rewrite or discard user-owned changes.**
- Commits follow the Lore protocol shape: imperative subject, body explaining the constraint and what was rejected, then `Constraint:` / `Rejected:` / `Confidence:` / `Scope-risk:` / `Directive:` / `Tested:` / `Not-tested:` trailers.
- **`Not-tested:` is mandatory and must be honest.** It is the most valuable line in the commit.

## 10. Enforcement — what is mechanical vs. reviewed

Rules that aren't enforced decay. This is the honest split.

**Mechanically enforced** (CI fails):

| Rule | Mechanism |
|---|---|
| No `unsafe` | `unsafe_code = "forbid"` |
| No unwrap/expect/panic/indexing outside tests | `[lints.clippy]` + `clippy.toml` test allowances |
| No stdout printing / `dbg!` | `clippy::print_stdout`, `print_stderr`, `dbg_macro` |
| No unbounded channels | `clippy.toml` `disallowed-methods` + `disallowed-types` |
| Dependency direction | `tests/architecture.rs` (Phase 1) |
| Licence / advisory / source policy | `cargo deny check` |
| Formatting, lints | `cargo fmt --check`, `cargo clippy -- -D warnings` |
| Function length, complexity | `clippy::too_many_lines`, `cognitive_complexity` (warn) |

**Reviewed by a human, not a tool** (state these explicitly in the commit):

- The extraction test (§2.2) and single-responsibility judgement.
- Whether a guard's negative test actually proves refusal.
- Whether `Not-tested:` is honest and complete.
- Whether a new dependency was worth it.
- Whether the README's claims match implemented behaviour.
- Whether provenance is genuinely independent.

Verification commands for every phase:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo deny check
```

## 11. Standing laws

These began as per-phase anti-patterns in the implementation plans. Each was restated in three to five separate phases, which is how five copies of one rule drift apart. They are stated **once, here**, and apply to every phase, past and future. A phase's own anti-pattern list now carries only what is specific to that phase.

1. **Terms and grants.** No provider, route, model, or fallback whose terms the user's own account does not actually grant. Quota laundering through reverse-engineered free tiers is out of scope permanently, not pending a better implementation. Breadth is exactly where this slips.
2. **Recall is a projection, never authority.** The append-only record and the working tree remain truth. Search indexes, summaries, embeddings, graphs, or other recall aids must be disposable, regenerable, provenance-carrying projections. They may improve discovery and context selection; they may not widen policy, approve an action, rewrite the record, silently become durable truth, or teach the runtime to grant authority from past success. Routing and trust ceilings remain declared and inspectable.
3. **Surfaces select; they do not act.** Onboarding, `/config`, the model and theme pickers, the fleet rail, and any guided surface added later may authenticate, scaffold declared files, and change selections. None of them may take a repo action, and none may reach a side effect that skips the gate the ordinary path would apply.
4. **Nothing in flight is widened.** No scheduled run, routing advance, subagent, council, extension, or wrap-up directive may widen policy, budget, approval tier, or credential scope — not for its own convenience, not to make automation smoother. Children inherit less, never more.
5. **The record is append-only; everything else is a projection.** Compaction, handoff, summarization, and fleet views derive from the durable transcript and never rewrite it in place. If a projection is insufficient, improve what is recorded — do not call a model to paper over a thin record.
6. **A directive is only as trusted as its source.** Text that reaches mjolnr from outside the session — a webhook body, an issue, a comment, anything a third party wrote — is *data about what someone wants*, never authority from the owner. It arrives framed as data, escaped so it cannot close mjolnr's own framing, and it cannot run unattended: full-auto is capped for it exactly as `carried_forward` caps it across a resume, and for the same reason. mjolnr does not attempt to detect a hostile directive and must not claim to; what it refuses is to confuse what someone asked for with what the owner authorised. Until triggers, every directive came from the person at the keyboard and this rule had nothing to govern — that is why it is newer than the rest.
7. **User-revertible state lives in diffable files.** Config, routes, personas, `SOUL.md`, skills, and extensions are files under `.mjolnr/` a human can read, diff, and revert. No hidden global mutable config blob, no settings store shadowing those files, no prose in SQLite. This is the safety case for admitting self-evolution at all.

Visual and interaction prohibitions are **not** here — they live in [`docs/tui-design.md`](./docs/tui-design.md), which is their contract. A border rule and a credential rule do not belong in the same list; flattening them teaches contributors that neither is serious.

## 12. Deviating from these rules

The rules are defaults, not dogma — but deviation is a recorded act, not a quiet one.

1. Prefer to comply.
2. If compliance is genuinely worse, write an ADR in `docs/adr/`: decision, constraints, alternatives rejected, accepted costs.
3. Note it in the commit and stop for review.

**Never** deviate silently, and never use `#[allow(...)]` to escape a prime directive (§1) or a security rule (§3). A local `#[allow]` for a narrow, justified case is fine — with a comment saying why, on the line, in that PR.

If a rule here is wrong, say so and change it deliberately. A standards file nobody believes is worse than none.
