# ADR-0014: Maps and the plan family coexist; the board projects both

**Status:** Accepted
**Date:** 2026-08-04
**Deciders:** Jerrik Christiansen
**Phase:** E5

## Context

An open planning question asked whether the map model and smed's plan family
should "merge or coexist", and recorded why the question was open: "Nothing has
been designed that tests whether the plan family's revision and stage model fits
a frontier-and-fog map."

The two models, as they actually exist:

**The plan family** (`src/core/plan.rs`) is a deterministic state machine over
one goal. `PlanWorkflow` carries `active_revision`, a `PlanStage` — `Idle`,
`QuestionPending`, `Proposed`, `Reviewed`, `Approved`, `IterateRequested`,
`Rejected`, `Handoff` — and vectors of proposals, reviews, approvals, and
handoffs. It is scoped within a session lineage, and `PlanApproval` is a durable
human act.

**The map model** is a graph over many decisions: a destination, a **frontier** of decisions takeable now, and
**fog** — decisions blocked on unresolved ones. Tickets carry a type and blocking
relationships hold between them. It is long-lived and spans sessions.

The design review that produced the map model says "those should be one surface,
not two." That is a statement about the **surface**, and this ADR reads it that
way. One board, two models, is consistent with it. One board, one merged model,
is a stronger claim that the review does not make.

## Decision

**They coexist. The map is a projection; it is not a new source of truth, and the
plan family is not folded into it.**

Specifically:

- The **plan family stays exactly as it is** — same state machine, same
  revisions, same approvals, same session scoping.
- A **map is derived** from three recorded sources: the plan family's durable
  stages and revisions, decision tickets (ADR-0015), and work items imported
  through §D6.
- The **frontier is a deterministic function over the blocking graph**, computed
  from that projection and re-derivable from the event log alone.
- The board renders the projection. It stores nothing that is not derivable.

## Why not merge

Three reasons, in descending order of how much they would cost to get wrong.

**1. Merging would make approval an input to the frontier.** The plan family's
stages carry `Approved` and `PlanApproval` — a durable human authorisation. The
frontier answers a different question: what is *decidable* right now. If plan
stages become map nodes directly, the frontier computation has to read stage,
and stage includes approval. At that point "what can I decide next" starts
depending on "what has been authorised", and the two collapse.

That collapse is the rubber stamp rebuilt one level up — the same failure the
council design names when it rejects a single aggregate verdict: "A single
aggregate verdict rebuilds the rubber stamp one level up." Keeping the
map a projection keeps *decidability* and *authorisation* in different types, so
neither can be read as the other by accident.

**2. Nothing has tested that the models fit, and a merge commits before the
test.** The plan family has revision semantics — a proposal is superseded, a
revision is active, reviews attach to a revision. A map has none of that; a
decision ticket is resolved once, is long-lived, and its resolution is
permanent. Merging means deciding today that revision semantics apply to
decision tickets, or that they do not, with nothing built that shows which.
A projection defers that commitment at almost no cost.

**3. The asymmetry of being wrong.** Coexistence that should have been a merge
leaves a projection layer to collapse later — additive work over derivable data.
A merge that should have been coexistence means unpicking durable event
semantics, on the largest slice in the plan, after work has been recorded against
them. `AGENTS.md`'s durability rules make the second direction the expensive one.

## What this preserves

- **`definition-of-done.md` §7's terminal state**, verbatim: "the board is a
  projection of recorded state rather than a second source of truth". A merged
  model would have made the board's own storage the question; a projection makes
  it a non-question.
- **The property that makes this smed's.** Every hosted board asks a human or a
  model what comes next; smed derives it from recorded state and shows its
  working. A derivation needs recorded state underneath it that it did not
  itself author.
- **Trust classes stay separate.** Imported work items are
  `externalUnverified`; plan and decision state is smed-owned. One merged model
  would have to carry a trust class per node anyway — the projection carries it
  per source, which is where it actually comes from.

## Consequences

- The board's data path is: recorded events → plan family / decision tickets /
  imported items → map projection → board. Every arrow is derivation. There is no
  write path from the board into the plan family that is not an ordinary
  plan-family command.
- **E5 depends on §D6 for imported items.** The projection boundary was designed
  before the integrations; GitHub and Linear producers have since landed without
  changing the board's ownership model.
- **`definition-of-done.md` §Open decisions 4** — "Does the task board own state
  or project it?" — is answered here as "project it" and exercised by the D6
  integration path.
- A decision ticket's resolution is defined separately, in ADR-0015, because it
  is the one place where a projection could accidentally acquire authority.

## Rejected alternatives

- **Merge into one model.** Rejected for the three reasons above; principally
  that it makes approval an input to decidability.
- **Two separate surfaces.** Rejected as contrary to the "one surface, not two"
  finding above and to the workflow: the owner should not have to know whether a thing is a plan node or a ticket to see it.
- **The board owning its own state, syncing both ways.** Rejected by
  `definition-of-done.md` §7 directly. It is also how a board becomes the thing
  that has to be reconciled after every disagreement.
