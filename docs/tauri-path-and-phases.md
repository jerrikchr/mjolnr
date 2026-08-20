# Fastest responsible path to Tauri

Status: **A0–C and D0–D8 have landed in bounded slices. The desktop client,
shared runtime bridge, search, review, repository, task, editor, and terminal
surfaces exist. D9–D12 and complete packaged dogfood journeys remain.**

Original date: 2026-07-28
Status updated: 2026-08-17

Related decisions:

- [`ADR 0003`](./adr/0003-shared-rust-core-tui-tauri-clients.md)
- [`ADR 0004`](./adr/0004-sveltekit-tauri-frontend.md)
- [`ADR 0006`](./adr/0006-bounded-integrated-developer-workspace.md)
- [`ADR 0007`](./adr/0007-obsidian-cyan-token-palette.md) (mockup tokens/palette)
- [`ADR 0008`](./adr/0008-remote-sync-from-the-last-fetch.md) (D5 remote sync)
- [`ADR 0009`](./adr/0009-mockup-layout-as-structural-target.md) (mockup layout, D2/D5/D7/D8)
- [`Tauri design system`](./tauri-design-system.md)
- [`Integrated workspace phases`](./integrated-workspace-phases.md)

## Outcome

Reach useful Tauri work immediately without creating a second agent runtime or
exporting false authority to the frontend. Ratatui remains a compact first-class
client. Tauri becomes the rich Orca-like workspace. Both clients consume one
authoritative Rust runtime.

The node canvas is important but additive. It is not a prerequisite for the
desktop workspace and it does not choose the frontend framework.

## Principles

1. One authoritative Rust runtime; Ratatui and Tauri are clients.
2. Client presentation state is reversible; governed authority is runtime and
   store state.
3. Start Tauri when the boundary is truthful enough, not after complete TUI
   parity or every future core workflow.
4. Ratatui receives deletion and retargeting, not more panel accretion.
5. The design system precedes product-surface proliferation.
6. Deferred work remains deferred until evidence makes it relevant.

## Decision

Build a restricted Tauri walking skeleton over a frontend-safe contract first.
Then add runtime-owned planning authority, close the TUI's plan heuristics, and
make planning the first complete cross-client workflow.

The frontend stack is **Svelte 5 + SvelteKit + TypeScript**, configured as the
static SPA documented by Tauri. This choice is based on the current
application's streaming, stateful UI, routing, accessibility, and component
system—not on possible Konva use. Svelte 5 compiles reactive dependencies into
targeted update effects, and its compiler accessibility warnings directly
support the design-system work.

Vanilla TypeScript was rejected because mjolnr already needs component
composition, lifecycle, reactive state, routing, accessibility, testable
primitives, and a durable design system. Avoiding a framework would move those
responsibilities into mjolnr-owned infrastructure.

shadcn-svelte is the starting source for selected primitives, not the design
system itself. mjolnr owns the resulting components, semantic tokens, behavior,
tests, and provenance. The Tauri client uses no SvelteKit server machinery.

Konva remains unevaluated. If a later canvas phase selects it, use raw Konva
behind a client-local adapter rather than making a framework binding part of
the core architecture.

## Phase sequence

Phases A0–C are implemented on `main`: runtime workflow authority, Ratatui truth
closure, cross-client planning, and the desktop workspace MVP.

This records code landing, not completion of the owner's live daily-session
test.

D0–D8 have since landed in bounded slices. This includes deterministic search,
review threads, governed repository controls, GitHub, Linear, Vercel and Supabase
task sources, the desktop editor, and terminal management. Remaining breadth and
live-journey
gaps are listed in the canonical table in
[`integrated-workspace-phases.md`](./integrated-workspace-phases.md), which is
canonical; do not infer release readiness from this summary.

### Phase A0 — Frontend contract, design system, and walking skeleton

#### Objective

Prove that Tauri can host `SmedRuntime` in-process while Svelte remains a
replaceable client. Establish the visual and interaction primitives before
building product surfaces.

#### Runtime contract

- Add bounded serializable `ClientSnapshot`, `ClientEvent`, and
  `ClientCommand` types in `src/core/client.rs`.
- Keep conversion, resynchronization, and hosting in
  `src/runtime/client_bridge.rs`.
- Keep Tauri commands and channels as glue. They own no projection, provider,
  policy, persistence, or tool logic.
- Use commands for bounded intent and a channel suited to runtime updates.
- Treat the snapshot as current truth; events provide ordered activity and
  history. Lag must trigger explicit resynchronization.

#### Design-system contract

- Implement the tokens, state vocabulary, primitives, accessibility rules,
  density, motion, and validation specified in
  [`docs/tauri-design-system.md`](./tauri-design-system.md).
- Build the component gallery before assembling the first workspace screen.
- Seed only the required primitives from shadcn-svelte, backed by Bits UI and
  restyled through mjolnr's tokens. Do not import its demo identity wholesale.
- Keep semantic governance states aligned across clients without forcing the
  desktop and terminal clients to share rendering code.

#### Allowed desktop surface

- open a project;
- create and resume a session;
- conversation and live activity;
- usage;
- existing approval and recovery state.

#### Explicit exclusions

- plan proposal, review, approval, and execution handoff;
- council output presented as verdict;
- applied or verified claims absent from an authoritative DTO;
- canvas or Konva;
- a second agent loop or policy engine.

#### Acceptance criteria

- The DTO layer does not expose internal `Arc`-heavy runtime types.
- `ClientCommand` cannot express “execute tool” or “call provider.”
- Lag causes explicit snapshot resynchronization rather than silent gaps.
- Cancellation reaches the runtime and emits exactly one terminal outcome.
- Tauri opens a project and creates or resumes a session.
- Conversation, activity, approvals, and recovery render from runtime truth.
- Architecture enforcement covers future frontend paths.
- No desktop surface can advance a plan or infer governed outcomes.
- The design-system gallery passes keyboard, focus, contrast, reduced-motion,
  resizing, and semantic-state review.
- A production bundle report and localized-update profile are recorded. If the
  chosen stack causes material regression against the bounded vanilla
  reference, stop and reconsider ADR 0004 rather than normalizing the cost.
- Every dependency is justified in the report and `THIRD_PARTY.md`.

### Phase A1 — Minimum runtime workflow authority

#### Objective

Replace plan-shaped prose and UI convention with append-only runtime truth.
Deliver the smallest state machine needed for questions, proposals, advisory
review, human approval, and handoff.

#### Contract

- Add `src/core/plan.rs` with typed IDs, question obligations, plan revisions,
  review evidence, `Approve`/`Iterate`/`Reject`, human approval, and handoff.
- Add append-only workflow events to `SmedEvent`.
- Reduce them into current plan state in the runtime.
- Persist the events through SQLite and checkpoint projection.
- Every modifying semantic command names its plan and revision.
- Model and council output remains advisory; plan approval remains human-only.

#### Acceptance criteria

- Event order is mechanically tested:
  `question → answer → proposal(vN) → review(vN) → approval(vN) → handoff`.
- Skipped, duplicate, stale, and superseded transitions fail closed.
- Approval of revision N is invalid once revision N+1 exists.
- Clean resume reconstructs exactly the same workflow state.
- Crash tests cover proposal, review, approval, and handoff boundaries without
  automatic retry.
- Runtime and client snapshots expose authoritative workflow state.
- No transcript string or client-local marker advances the workflow.

Small concrete work may remain direct. Ambiguous or medium work gets a short
question-and-plan path. Large, architectural, destructive, or high-risk work
gets ordered architecture and critical review before human approval.

### Phase A2 — Ratatui truth closure

#### Objective

Make Ratatui render A1 truth and delete its plan pseudo-authority. This is not a
new TUI feature phase.

#### Scope

Only plan-related code in `src/tui/app.rs`, `src/tui/reducer.rs`,
`src/tui/plan_surface.rs`, `src/tui/workspace_types.rs`, `src/tui/keymap.rs`,
and directly related tests.

#### Acceptance criteria

- `/plan` no longer means only “set policy to read-only.”
- Plan approval is not inferred from policy plus visible plan steps.
- `[DONE:n]` or other assistant prose carries no progress authority.
- Plan, work, and attention views consume snapshot truth.
- False-positive plan frames and stale approvals have negative tests.
- Approval and recovery remain separate interactions.
- No shell, launcher, navigation, theme, or new-panel redesign enters the diff.

### Phase B — Cross-client planning vertical slice

- Requirements, revisions, reviews, human approval, and handoff render from
  client DTO truth.
- Tauri cannot approve a stale or superseded revision.
- Ratatui can resume and render the same workflow.
- Council dissent remains visible and cannot create authority.
- Direct, short-plan, and reviewed-plan paths are visibly distinct.

### Phase C — Orca-like Tauri workspace MVP

- Persistent work navigation and selected work object.
- Conversation, Plan, Changes, Verify, and Attention surfaces.
- Authoritative approval and recovery states.
- First-launch and onboarding journey.
- Responsive layout, keyboard navigation, and design-system conformance.
- No canvas, custom graph editor, or frontend inference of governed outcomes.

### Phases D0–D12 — Integrated developer workspace

The next round expands the desktop through explicit, separately checkpointed
capabilities:

1. authority/trust contracts;
2. persistent hierarchy and split layouts;
3. governed worktree and child-run control;
4. exact diffs and line-level review;
5. deterministic workspace search;
6. governed Git;
7. GitHub, Linear, Vercel and Supabase task sources;
8. file explorer and code editor;
9. terminal multiplexing;
10. external CLI-agent compatibility;
11. bounded browser/design mode;
12. SSH workspaces and account profiles; and
13. a daily-driver integration checkpoint.

The full objectives, implementation boundaries, acceptance criteria,
dependency order, and verification contract are canonical in
[`integrated-workspace-phases.md`](./integrated-workspace-phases.md). Do not
implement from this summary alone.

## Deferrals

Phase 33B remains queued and non-blocking. Reconsider it after the first Tauri
workspace MVP or as an independent runtime lane after A1 when a concrete
user-facing need exists.

The following remain outside D0–D12 unless a separate decision changes them:

- graph persistence, semantic recall, or multi-language graph expansion;
- graph token-savings work beyond a bounded evaluation;
- Konva or a node canvas;
- hosted execution;
- a daemon or RPC boundary.

## Verification

Every implementation phase runs:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo deny check
```

Phase-specific evidence:

- A0: DTO round trips, command-allowlist refusals, lag/resync, cancellation,
  create/resume smoke, component gallery, accessibility checks, bundle report,
  and localized-update profile.
- A1: event-order state machine, persistence round trips, checkpoint/resume,
  crash matrix, and stale-revision refusal.
- A2: semantic TUI frame tests and diff-scope audit.
- B: cross-client resume and stale-approval integration tests.
- C: desktop onboarding/session journey, approval/recovery visual QA, and a
  packaged macOS smoke test.
- D0–D12: use the phase-specific evidence and expanded Rust/desktop/package
  verification contract in
  [`integrated-workspace-phases.md`](./integrated-workspace-phases.md).

Stop at each phase checkpoint. A failed criterion blocks the next phase.
