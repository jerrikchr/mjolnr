# smed — definition of done

*Owner's specification, 2026-07-31. The finish line for smed as a product, written
from the workflow it exists to serve.*

## What this document is

Every other planning document in this repository answers *what should be built
next*. None of them answers *when is it finished*. That absence is why the
backlog reads as infinite: phases are ordered breadth-first against a list that
grows each time something is learned, and a breadth-first plan against an open
backlog has no terminal state by construction.

This document supplies the terminal state. It is written from the owner's own
workflow, because the owner is smed's first and most demanding user, and a
harness that serves that workflow completely is a harness that is finished enough
to be judged.

**This document does not renumber, reorder, or retire any phase.** It is the
target the phase plan will be measured against, in a later pass. Where it names a
capability that no phase covers, it says so and stops there.

---

## The finish line, in one sentence

> smed runs the owner's entire working day — greenfield and existing projects,
> planning through review through merge — across subscription-funded models, on
> owned hardware, without the owner dropping to another tool.

Two readings of that are wrong and worth foreclosing:

- It is **not** "smed can fix a bug." That is the narrowest path *through* the
  workflow and a sensible first thing to exercise, but shipping only that would
  leave the product undefined.
- It is **not** "every phase in every roadmap is closed." Several queued phases
  serve users who are not the owner, or hedge against futures that may not
  arrive. Those are legitimate work and they are not this.

Done is: **the whole workflow below runs, end to end, for real work.**

---

## The workflow

Nine stages. Each states what *done* means for that stage, so the finish line is a
conjunction of checkable claims rather than a feeling.

### 1. Setup and identity

The owner configures smed once and does not fight it again: providers connected,
models mapped to the work they are good at, and smed's own standing rules, roles,
personas, and Soul written down.

**Done when** a fresh machine reaches a working, personalised smed through a
guided flow, and every artifact it produces is a diffable file the owner can edit
by hand afterwards.

### 2. Provider and model assignment

Work is routed to the model that suits it, deterministically, not by asking a
model to choose. The owner's intended shape:

| Work | Model |
|---|---|
| Heavy logic, backend architecture | Opus 5 |
| Backend, second opinion / alternate approach | Kimi K3 |
| Frontend | GLM 5.2 |
| Breadth, cheap reasoning, wide scans | Gemini 3.6 Flash / 3.1 Pro |
| Small writing, local, private | Gemma 4 E2B via LM Studio |

**Amended 2026-07-31 — a second axis: token volume × leverage.** Routing on *kind
of work* alone can put the most expensive model on the highest-volume work, which
is the wrong failure under this funding model. Because access is subscription-based,
tokens are **time rather than money** — a burnt window cannot be bought out of, only
waited out. So volume matters independently of task type:

| Work | Volume | Leverage per token | Implication |
|---|---|---|---|
| Interview, planning, council | Low | **High** | Worth the strongest model |
| Discovery, review | Medium | High | Strong model, bounded output |
| Implementation | **High** | Lower per token | Cheaper models; local where it fits |

This is also the mechanical argument for planning heavily before executing:
planning is low-volume — one session, small context, mostly prose — while
stumbling is high-volume, in long contexts, retries, re-reads, and wasted fan-out.

**Done when** routes and roles express both axes, a session honours them without
being told again, and `/model` shows only models actually discovered from
connected providers.

### 3. Greenfield: interview → PRD → reviewed plan

Plan mode interviews the owner until a PRD exists. Council reviews the PRD. The
output is an implementation plan sliced into units of work that can be handed out
without the units colliding.

**Done when** the interview produces a PRD the owner did not have to write, the
council's dissent survives into the record verbatim, and the resulting plan is a
durable artifact with revisions and approvals rather than a message in a
transcript.

### 4. Existing project: discovery

Pointed at an unfamiliar repository, smed learns it before proposing anything —
structure, conventions, test and build commands, where the risk concentrates — and
reports what it found. Having learned it, it can *suggest* a model assignment
appropriate to that project and the providers currently available.

**Done when** discovery is a bounded, deterministic, resumable pass whose output is
a durable artifact; and its model-assignment output is a **proposal the owner
accepts or edits**, never a route that assigns itself.

### 5. Execution: a team, not a soloist

Sliced work is handed to agents. Two shapes, both legitimate: a single agent
driving a goal to completion, or fan-out across isolated children who do not trip
over each other.

**Done when** both shapes run under one authorisation model, each child works in
its own worktree on its own branch, nothing merges because an agent decided it was
finished, and width comes from an authorisation the owner granted rather than a
constant someone raised.

### 6. Review loops

Work arriving from agents is reviewed by models holding a review role, against the
plan it came from. Findings are anchored to the diff they were written against.

**Done when** a review round-trips without the owner reading every line — the owner
reads *findings*, not diffs — and a stale finding says it is stale rather than
pointing confidently at a line that moved.

### 7. Task and issue integration

GitHub issues and pull requests, Linear issues, Vercel deployments, and Supabase
projects appear as work in smed, on a board the owner can move things around, and
(only for GitHub) state syncs back. Opening a repository and starting on a known bug
is a first-class entry point, not a special case. Vercel and Supabase are bounded
read-only `TaskSource`s (`api.vercel.com`, `api.supabase.com`) with the same
`TaskId` charset, bounded reads, `Secret`-redacted provenance, and `HashMap`-held
registry — no PR destination, `submit_change` refuses typed `Unavailable`.

**Done when** the board is a projection of recorded state rather than a second
source of truth, and external text — issue bodies, PR descriptions, review
comments — is treated as untrusted data that can never approve a tool or widen a
policy.

### 8. Seeing the system

The knowledge graph is visible, not merely queryable — the codebase as something
the owner can look at and navigate, alongside the transcript, the diff, and the
board.

**Done when** the visualisation is a view over the deterministic graph, and
navigating it is a way to *drive* work rather than a picture beside it.

### 9. Running out of quota without losing the thread

The binding constraint is not money, it is **subscription windows**. See below.

**Done when** an approaching limit is a routing event rather than a stopped
session.

---

## The funding model, and why it changes the budget design

This is a first-class constraint, not a footnote, and it inverts an assumption
present in the current budget design.

**The owner does not pay for direct API usage on any provider.** Access is via
subscription coding plans — Claude, Codex, Antigravity, z.ai's plans for GLM and
Kimi — plus local models that cost nothing but time. Consequences:

1. **Budgets denominated in dollars are meaningless here.** The real limits are
   rolling windows: five-hour, weekly, monthly, per provider, each with its own
   reset. A budget smed can actually enforce must be expressed in the units the
   provider actually meters.
2. **Exhausting a window is normal, not exceptional.** It will happen mid-task,
   repeatedly, by design — that is what a subscription plan is. A harness that
   treats window exhaustion as an error condition will spend most of its life in
   an error condition.
3. **Therefore the response to exhaustion must be continuation, not a halt.** When
   a window is about to close, work should move to another model that is still
   inside its own window — **carrying the plan, the status, and the evidence, but
   not the entire context window.** Re-sending a full transcript to a fresh model
   spends the new window on catching up, which defeats the point of moving.

**How much of this exists.** More than expected. `QuotaWindow` carries `resets_at`;
`QuotaReserveStatus`, `QuotaReserveBasis`, and `QuotaReservePhase` exist in
`src/core/continuation.rs`; `/handoff` plus compact resume already carries a
status artifact and bounded recent turns to a *different model* while the full
history stays in SQLite. Phase 10 built the hard half.

**What exists now.** E0/E1 closed these gaps: provider-specific concurrent
windows are recorded with their reported or explicitly inferred basis, the
provider response path automatically persists a boundary and starts the safe
continuation/handoff path, and `/usage` labels reported, polled, configured, and
unavailable data honestly. The runtime and provider-contract tests exercise
those distinctions without inventing data a provider did not return.

The remaining quota observations are provider-specific live-use questions — for
example, whether a Codex secondary window ever activates and whether a monthly
window is exposed. smed reports only data it actually receives; it does not
invent a window to make the shape look complete.

**The standing risk, recorded once.** Subscription OAuth is not an API contract.
This repository already says so about the subscription routes it ships, and
providers can change or close such routes without notice. smed's architecture is
provider-agnostic and therefore survives any single closure; the *owner's
configuration* would not.
The finish line therefore requires that losing a subscription route degrades to a
narrower smed rather than a broken one — routes fall back, `/auth` states plainly
what is unavailable, and no capability silently disappears.

---

## State of play

Measured against the nine stages, not against the phase list.

| Stage | State |
|---|---|
| 1 Setup and identity | **Built** — `smed init`, roles, personas, Soul |
| 2 Model assignment | **Built** — deterministic routes and roles |
| 3 Interview → PRD → reviewed plan | **Built** — bounded owner interview produces a durable PRD, links its advisory council review, synthesizes a durable plan, records human approval and handoff, and replays the chain on resume |
| 4 Discovery | **Built** — E3's deterministic code graph plus E4's bounded discovery pass writing a non-overwriting OKF bundle, including `model-assignment.md` |
| 5 Execution | **Built** — subagents, worktrees, clamped authority, envelopes, governed `fetch`/`push` (D5-5), merge-from-upstream (D5-6), bounded history, verified clone, clean-tree rebase with explicit recovery, and GitHub PR creation (D6-4) with recorded remote revision and verified local commit/branch |
| 6 Review loops | **Built** — anchored review threads and read evidence landed with §D3; council distribution, artifact review, and deterministic amended-artifact composition landed with E6 |
| 7 Task/issue integration | **Built** — the E5 board, GitHub fetch/import/refresh/PR producer path, Linear fetch/comment sync-back, Vercel deployments and Supabase projects as bounded read-only `TaskSource`s (`api.vercel.com`, `api.supabase.com`, `Secret`-redacted, `TaskId` charset, `HashMap` registry), bounded batch import, model-intersection containment, pinned GitHub comment sync-back, desktop Board task/PR controls, and desktop review-comment composition are live |
| 8 Graph visualisation | **Built** — the E7 interactive canvas provides bounded search, provenance, navigation, and editor-driving selection over the deterministic graph |
| 9 Quota continuation | **Built** — reserve and cross-model handoff existed from Phase 10; E0/E1 gave the trigger machinery correct data, so window-boundary continuation, `/usage` window reporting, and Google pools all have real sources |

All nine stages are now built against this definition. The remaining queued E/D
work is deeper desktop breadth and packaging validation, not a missing stage in
the owner's defined workflow.

---

## Explicitly outside this finish line

Named so the current completion claim stays bounded. These capabilities may be
designed and built later; their absence does not make the nine-stage workflow
unfinished today.

- **OS-level sandboxing.** The current product is a policy gate, not a security
  sandbox. Any future containment claim needs a separate design, negative tests,
  and platform-specific verification.
- **Windows.** Unsupported until tested on Windows.
- **Multiplayer / org-level operation.** The current finish line is local-first
  and single-owner. Team services may build on the same runtime contracts later.
- **Semantic recall across sessions.** Durable history and explicit handoffs are
  built; automatic semantic recall is not. Any future index, summary, embedding,
  or graph remains a disposable projection over authoritative sources.
- **Every remaining queued phase.** Some serve users who are not the owner. They
  are not cancelled; they are simply not what "done" means.

---

## Open decisions this document surfaces

These were implementation questions, not blockers. They are now settled by the
landed slices:

1. **Budget denomination:** provider-specific reported windows and explicitly
   labelled configured estimates; no dollar fiction and no invented monthly
   window.
2. **Successor selection:** deterministic routes choose the successor; the owner
   edits the route configuration rather than approving a model-generated choice
   at the boundary.
3. **Discovery shape:** an explicit bounded session command (`/discover`) writes
   a durable OKF bundle under `.smed/discovery/run-*`; the runtime exposes only a
   bounded projection of that artifact.
4. **Board ownership:** the board projects durable smed events. GitHub/Linear
   fetch, refresh, revision-pinned acts, and replay tests exercise that boundary;
   remote text remains untrusted data.
5. ~~**The licence.**~~ **Settled** on 2026-07-31: Apache-2.0. Kept in
   the list, struck through — a decision recorded as open long after it was made
   is how the list loses its authority.
