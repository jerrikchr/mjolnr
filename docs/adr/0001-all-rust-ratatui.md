# ADR 0001 — All-Rust with Ratatui

- **Status:** accepted for the Rust governance core; Ratatui-only product-surface
  assumption superseded by ADR 0003
- **Date:** 2026-07-15
- **Decider:** Jerrik
- **Context:** Initial runtime and terminal-stack selection

## Decision

mjolnr is written entirely in Rust. The TUI uses Ratatui and crossterm. mjolnr owns its agent loop and speaks directly to provider HTTP APIs. One process, no RPC boundary, no daemon in the MVP.

This remains the correct record of the MVP stack decision. On 2026-07-28,
ADR 0003 selected a Tauri application as the rich client while retaining
Ratatui as a first-class terminal client. The agent loop and governance core
remain Rust-owned; “all-Rust” no longer constrains every future presentation
component.

## Context

mjolnr is a local-first terminal AI coding harness that owns its agent loop, tools, policy gates, sessions, and skills. The candidate stacks were TypeScript (Bun), Go, Rust, and a split TypeScript-core-plus-Rust-TUI hybrid.

TypeScript has the broadest agent-SDK ecosystem, but mjolnr owns its harness and
integrates through documented provider contracts. That makes the language choice
primarily about distribution, long-lived process behavior, and the correctness
of the guard layer rather than access to another harness's SDK.

## Rationale

**Speed is explicitly not the reason.** This workload is I/O-bound: a step takes seconds and nearly all of it is waiting on a provider API. Rust does not make a model answer faster, and any argument resting on throughput is optimising the fraction of runtime that does not matter. Recorded plainly so nobody later "optimises" on a premise that was never true.

The reasons that do hold:

1. **Distribution.** A single static binary, no runtime prerequisites. The counter-example is concrete: Hermes' installer ships `uv`, Python 3.11, Node.js, ripgrep, ffmpeg, and a portable MinGit (~45MB), and its README carries a troubleshooting section for Windows Defender quarantining `uv.exe` as malware, with attestation instructions so users can prove their own installer is not a virus. That is the interpreted-runtime distribution tax, documented by its own maintainers. mjolnr's users run one binary.
2. **Typed guards.** mjolnr's core promise is that the model proposes and deterministic code disposes (`AGENTS.md` §1). Guards, typed refusal codes, tool tiers, and path containment are exactly where a strong type system earns its cost, and exactly where correctness beats iteration speed.
3. **A supervised long-lived process** is Rust's home turf. Post-MVP scheduling wants a daemon; Hermes' 3,935-line scheduler with a hand-rolled read/write lock and thread pools is partly Python's GIL showing through.
4. **Owner preference.** Jerrik selected Ratatui and wants to own and maintain a Rust product. For a team-first tool, the maintainer's willingness to live in the codebase is a legitimate engineering input, not a rounding error.

## Alternatives rejected

**TypeScript/Bun core (with Ink or OpenTUI).** Fastest iteration, best SDK ecosystem, and `bun build --compile` addresses distribution. Rejected: the SDK gravity argument dissolves once mjolnr owns the harness, and it does not deliver the typed-guard benefit. OpenTUI was researched and is a genuine option for a TUI-heavy client; it is a reference, not a dependency.

**Rust TUI over a TypeScript RPC core.** Rejected because it introduces a second
toolchain and a process boundary while putting the strongest type guarantees on
the client side of the policy gate. The native path stays in one process unless
a future deployment requirement justifies a recorded boundary.

**Go.** Best-in-class TUI ecosystem (Bubble Tea), single static binary, excellent concurrency for the eventual daemon. Rejected in favour of Rust's stronger type system for the guard layer and the owner's stated preference. This was the closest call.

**Python.** Rejected on distribution alone; see the Hermes evidence above.

## Consequences

Accepted costs, stated so they are not rediscovered as surprises:

- Rust has no first-party provider SDKs, so mjolnr owns more protocol code. Mitigated by narrow adapters written against documented REST contracts, redacted fixtures, and property-tested parsers.
- Five provider adapters and streaming state machines are ongoing maintenance. Mitigated by one provider end-to-end (Phase 3) before breadth (Phase 7).
- Ratatui provides less than OpenTUI for coding-specific widgets, so mjolnr builds more of them. Mitigated by delivering semantic components incrementally; correctness first, syntax polish later.
- All-Rust will take longer than composing existing TypeScript packages. Accepted deliberately.

Structural consequences:

- The TUI is a client of `SmedRuntime`, even though there is no process boundary to enforce that. `tests/architecture.rs` enforces it instead (`AGENTS.md` §2.1). Without a process split, discipline is the only thing preventing the TUI from reaching into a provider — so it is mechanised rather than trusted.
- One Cargo package, module boundaries kept crate-extractable (`AGENTS.md` §2.2).
- `unsafe` is forbidden outright. A memory-safety incident would forfeit the main reason for choosing this stack.

## Revisit if

- Driving provider APIs or the Agent Skills standard from Rust proves materially worse than projected (the risk flagged in ).
- A post-MVP client (web dashboard, mobile approval surface — ) forces a real RPC boundary. That would reopen the core-language question honestly, and the enforced `tui → runtime → core` direction is what keeps that door open.
