# ADR 0003 — Shared Rust core with Ratatui and Tauri clients

- **Status:** accepted
- **Date:** 2026-07-28
- **Decider:** Jerrik
- **Context:** `docs/ux-workspace-direction.md`
- **Expanded workspace boundary:** [`ADR 0006`](./0006-bounded-integrated-developer-workspace.md)
- **Supersedes:** the Ratatui-only product-surface assumption in ADR 0001.
  ADR 0001's Rust governance-core decision remains accepted.

## Decision

mjolnr keeps its Rust-owned agent loop, tools, policy gates, durable record,
provider adapters, and semantic command/snapshot boundary.

Ratatui remains a supported, terminal-launched client. It will be repaired and
made compact and truthful, but it is no longer expected to imitate a full
desktop workspace by accumulating permanent panels.

The Tauri application is the selected rich interface and now exists in
`desktop/`. Its remaining development continues to preserve mjolnr's governance,
attention, recovery, and evidence contracts. The Tauri frontend is a client of
the Rust runtime, not a second agent engine.

A node canvas is additive and later. Konva is the current likely renderer for
that surface, but neither Konva nor the canvas is part of this ADR's committed
implementation scope.

## Context

The Ratatui client proved the governed loop and remains valuable for terminal
users, remote work, recovery, and low-overhead operation. Recent UX work also
showed its limit: translating every desktop-workspace concept into another
always-visible rail, tab, or panel made the interface denser without making the
underlying runtime state more authoritative.

Orca demonstrated that mjolnr's richer product vision benefits from a spatial,
onboarding-friendly desktop surface. HumanLayer and Herdr remain useful
references for decision flow and attention management. The canvas idea matters,
but it does not define the primary product or justify delaying the main
workspace.

## Boundary

```text
Ratatui client ─┐
                ├─ semantic commands / runtime snapshots ─ Rust runtime
Tauri client ───┘                                      ├─ core contracts
                                                       ├─ policy and tools
                                                       ├─ providers
                                                       └─ append-only store
```

Both clients may maintain reversible presentation state such as selected
surface, scroll position, filters, and window layout. Neither may infer or own
authoritative plan approval, tool completion, verification, recovery, or
execution state.

The first Tauri implementation may remain in-process. A daemon or RPC boundary
is not implied by this decision and requires a later ADR if needed.

## Consequences

- Core/runtime/client separation must be completed before rich Tauri work.
- New product workflows are implemented once in runtime/core and rendered by
  both clients.
- The TUI receives restraint-oriented repair: preserve the governed loop,
  remove or contextualize low-value chrome, and stop adding panels merely
  because a capability exists.
- Desktop onboarding, persistent workspace navigation, review surfaces, and
  later canvas interaction can use web UI primitives without moving authority
  into the frontend.
- “All-Rust” now describes the governance/runtime substrate, not necessarily
  every presentation component.

## Alternatives rejected

**Continue Ratatui as the only rich interface.** Rejected because the product
direction now includes non-developer onboarding, persistent spatial work
objects, rich review, and a later node canvas. Ratatui remains useful, but
forcing all of those concerns into it has already increased density.

**Replace Ratatui with Tauri.** Rejected because the terminal client remains a
valuable first-class surface and a strong test that runtime behavior is not
coupled to desktop presentation.

**Build the node canvas first.** Rejected because it is additive. The primary
value is the governed workspace and its planning, review, execution, and
attention flows.

**Run a separate TypeScript agent engine behind Tauri.** Rejected because it
would duplicate mjolnr's central governance boundary and make client behavior a
source of authority.

## Revisit if

- Tauri cannot consume the Rust runtime without weakening cancellation,
  backpressure, secret handling, or deterministic gates.
- A required desktop capability forces a process boundary; record that boundary
  separately rather than smuggling it into the frontend.
- Konva is selected for the canvas after a focused renderer evaluation.
