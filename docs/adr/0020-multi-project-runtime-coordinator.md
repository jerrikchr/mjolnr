# ADR 0020: Multi-project runtime coordinator

**Status:** Proposed

**Date:** 2026-08-21

## Context

mjolnr's current desktop composition owns one `Runtime`, one `SessionState`, one
workspace root, one write lease, and one client update stream. The durable store
can contain sessions from several projects, but the live client can operate only
one project/session context at a time.

That is insufficient for the owner's normal workflow: several projects may be
active at once, with work continuing in one project while another is visible.
Adding project rows to the sidebar without changing runtime ownership would be
misleading; commands, approvals, repository reads, and provider turns would
still target the one active root.

## Decision

The desktop client will introduce a runtime coordinator that owns one isolated
mjolnr runtime context per active project. Each context contains its own runtime,
client bridge, workspace root, session lease, transcript projection, and live
execution state. The coordinator owns selection and routes commands only to the
selected context.

Inactive contexts remain alive and governed. They may continue an authorized
run, but their events are buffered as bounded durable/runtime projections until
the user selects that context. No context may widen policy, budget, approval, or
credential scope because it is inactive or running in the background.

The desktop update stream becomes context-tagged. Switching contexts cancels
the previous frontend subscription, attaches the selected context, and emits a
fresh snapshot before accepting interactive commands. Shutdown closes every
context and waits for each bridge to flush and release its lease.

## Constraints

- The Rust runtime remains the authority; the coordinator is composition glue.
- A project context has at most one live session lease until the runtime itself
  gains a durable multi-session actor model.
- An uncertain context close must retain the existing explicit reclaim path.
- Shared SQLite storage remains append-only and all project identity checks stay
  in the runtime/store boundary.
- The Tauri frontend must never select a project by changing a local root string;
  selection is a coordinator command with an acknowledged snapshot.

## Alternatives rejected

- **Sidebar-only multi-project rows:** rejected because it would present several
  projects while all effects still target one runtime root.
- **One runtime actor with mutable project maps:** rejected because it would mix
  session state, leases, recovery, and cancellation domains without a clear
  ownership boundary.
- **Independent frontend-only transcripts:** rejected because the Tauri client
  must remain a projection and cannot become a second agent loop or source of
  session truth.

## Phased delivery

1. Add the coordinator and context-tagged active snapshot/update boundary while
   preserving the current single-context test seam.
2. Route open/switch/resume/dispatch through the selected context and project
   the real project list in the sidebar.
3. Keep inactive contexts running with bounded attention/event projections.
4. Close all contexts deterministically and add crash/restart coverage for every
   lease state.

## Verification

The feature is not complete until tests prove context isolation, provider/model
selection isolation, approval isolation, background cancellation, context
switch resynchronization, clean multi-context shutdown, and explicit recovery
after an uncertain close.
