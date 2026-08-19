# ADR 0006 — Bounded integrated developer workspace

- **Status:** Accepted
- **Date:** 2026-07-29
- **Decider:** Jerrik Christiansen
- **Related:** [`ADR 0003`](./0003-shared-rust-core-tui-tauri-clients.md),
  [`integrated-workspace-phases.md`](../integrated-workspace-phases.md)

## Decision

smed’s Tauri client will grow beyond the initial governed-session workspace
into a bounded integrated developer workspace. The roadmap includes:

- persistent project, worktree, session, subagent, and parent/child work
  navigation;
- exact changes, review notes, deterministic search, source control, and
  GitHub/Linear task integrations;
- a file explorer and code editor;
- terminal tabs and splits;
- external CLI-agent interoperability;
- an embedded browser with an opt-in design-inspection mode;
- SSH-backed remote workspaces; and
- explicit provider and integration account switching.

These are smed capabilities, not a change in smed’s authority model.
Authoritative agent execution, policy, approval, recovery, and verification
remain Rust-owned. GitHub and Linear are task sources attached to smed work
items; neither becomes smed’s product identity or source of authority.

## Trust classes

The workspace must distinguish three execution classes:

1. **smed governed.** A smed-native model run. Every model-proposed side
   effect passes smed’s deterministic policy, durability, approval, execution,
   and verification path.
2. **Operator controlled.** A terminal, editor, Git, or browser action directly
   initiated by the human. smed may record the action and its result, but must
   not describe it as model-governed.
3. **External unverified.** An attached or smed-hosted third-party CLI agent.
   smed may isolate, observe, stop, and review it, but cannot claim its internal
   side effects were governed unless a specific adapter proves that every
   effect is forced through smed’s tool proxy.

External changes become eligible for smed’s normal review, staging, and
verification surfaces only through an explicit import boundary. Observation
alone never turns external output into a verified smed result.

## Security and authority boundaries

- Tauri and Svelte remain clients. PTY hosting, Git operations, filesystem
  mutation, browser capture, integration API calls, SSH, and credential
  selection live behind typed Rust commands.
- Client commands name stable object IDs and expected revisions. Stale commands
  fail closed.
- Model text cannot directly drive a terminal, editor, browser, Git, task
  integration, or SSH action.
- Operator-controlled surfaces are visually distinct from model-governed
  surfaces.
- External task text is untrusted data. Starting a smed session from an issue
  or pull request requires an explicit human action and records its source.
- Credentials remain in owner-only credential files or environment boundaries.
  They never enter argv, SQLite, logs, transcript text, browser-injected
  scripts, or frontend snapshots.
- Browser design mode defaults to explicitly allowed local development origins.
  Remote-origin inspection requires a separate opt-in and cannot silently
  expose cookies, storage, or authenticated DOM content to a model.
- SSH requires host-key verification, bounded reconnect behavior, and the same
  trust-class labels as local work.

## Why

The initial Tauri workspace proves smed’s governed loop, but a daily-driver
desktop application also needs the surrounding review and development context.
Forcing users to switch between smed, a terminal, an editor, a browser, Git,
and task systems weakens the workspace experience and makes the durable work
hierarchy harder to understand.

The rejected alternative was not “rich workspace versus no rich workspace.”
It was an unbounded IDE that quietly blurred human actions, arbitrary external
agents, and smed-governed execution. Explicit trust classes permit the useful
workspace while keeping claims honest.

## Consequences

- The desktop scope is materially larger and will be delivered as independent
  checkpoint phases rather than one IDE rewrite.
- The Rust client contract must expand before each new surface becomes
  actionable.
- Some operator-controlled capabilities may exist without being available to
  a model.
- “Every CLI agent” means a generic, clearly labelled compatibility surface.
  It does not mean smed can promise identical resume, approval, or tool-proxy
  semantics for every third-party CLI.
- New editor, PTY, browser, GitHub, Linear, and SSH dependencies require
  focused evaluation, licence review, `THIRD_PARTY.md` updates, and removal-cost
  notes in their reports.

## Alternatives considered

### Keep smed as a session viewer only

Rejected because it leaves the daily development loop fragmented and does not
deliver the workspace direction the product owner selected.

### Make arbitrary CLI agents first-class governed smed agents

Rejected because smed cannot truthfully guarantee control of an external
agent’s internal side effects. A future adapter may earn the governed label
only after enforcing and testing a complete tool-proxy boundary.

### Build all IDE capabilities directly in Svelte

Rejected because it would move process, filesystem, Git, credential, and
network authority into the presentation layer.

### Let GitHub or Linear own the work hierarchy

Rejected because smed must also represent local directives, resumed sessions,
branches, subagents, offline work, and non-integrated repositories.

## Revisit if

- a third-party CLI exposes a documented tool-proxy protocol that can prove
  smed controls every side effect;
- Tauri’s embedded-webview model cannot isolate design inspection safely;
- remote work requires a daemon or RPC boundary, which must receive its own
  ADR; or
- the integrated surfaces prevent smed’s core governed workflow from
  remaining independently usable.
