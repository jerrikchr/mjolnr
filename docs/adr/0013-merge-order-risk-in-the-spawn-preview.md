# ADR-0013: Merge-order risk belongs in the spawn preview, and needs declared scope to exist at all

**Status:** Superseded 2026-08-05 — see §Supersession
**Date:** 2026-08-04
**Deciders:** Jerrik Christiansen
**Phase:** E7

## Supersession, 2026-08-05

**This ADR is wrong and its subject dissolved.** Recorded rather than deleted,
because the reasoning that produced it is the useful part.

The decision below treats merge-order risk as something to *compute* before a
fan-out runs. Two findings retired that framing on the day after it was written:

1. **Git already answers it exactly.** `git merge-tree --write-tree` performs a
   real conflict-detecting merge in the object database — no working tree, no
   index, no HEAD — and names the conflicting paths. It answers the ordering
   case too, by merging one branch and testing the next against the resulting
   tree. Verified against git 2.43. So there was never a prediction to make.
2. **The order is a decision, not a derivation.** Children work in parallel and
   the orchestrator sequences the results when they land — stacked, in an order
   a human or the orchestrator chose. With the order chosen, there is no risk to
   analyse; git enforces the order and reports conflicts when they occur.

What follows from that: **no declared path scope, no new `spawn_subagent` field,
no graph blast-radius intersection, no unknown-versus-no-risk distinction, and no
spawn-time analysis of any kind.** The placement argument below — that risk
belongs where a human authorises a fan-out — was reasonable and is moot; spawn
time is too late to re-slice the work and too early to measure anything.

The remaining question is not risk but **capability**: mjolnr has no `push`,
`pull`, `fetch`, `clone`, or `rebase`, and `submit_change` refuses, so the merge
path terminates at the local `main`. Ordering PRs is an optimisation above a
floor that does not exist yet.

---

## Context

The E7 plan separates two things deliberately: the graph *view* is
exploration, and merge-order risk is *analysis*. It then leaves the analysis
unplaced, in open decision 1 — "review surface, graph view, or spawn preview?" —
with a hedge attached: "It is arguably an input to authorising a fan-out, which
would place it earliest."

The bounded graph pane states in terms that it "does not infer merge-order
risk". So the placement question remained separate from graph rendering.

Three candidate homes, and what each implies about *when* a human learns the
risk:

| Home | When the human sees it | What they can still do |
|---|---|---|
| Review surface | After the children have run | Unpick finished work |
| Graph view | Whenever they happen to look | Nothing in particular; it is not attached to a decision |
| Spawn preview | Before authorising the fan-out | Not fan out that way |

## Decision

**Merge-order risk is surfaced in the spawn preview**, as part of what a human
reads before authorising a fan-out. The graph view may echo the same query
read-only, but the graph view is not where the analysis lives.

**And it is computed only from declared path scope.** Where a child does not
declare its scope, the preview says the risk is **unknown** — never that there is
none.

## Why the spawn preview

`definition-of-done.md` §5 fixes the terminal state for execution: "width comes
from an authorisation the owner granted rather than a constant someone raised."
Merge-order risk is precisely an input to that authorisation. Two children that
will collide on merge is a fact about the fan-out being proposed, and the only
cheap moment to act on it is before the children run.

The other two placements both arrive too late or too loose:

- The **review surface** shows it once the work exists. At that point the risk
  has already been taken and the remaining moves — serialise the merges, discard
  a branch, re-run a child — are all expensive. Review is where a *realised*
  collision is handled; it is not where the risk is decided.
- The **graph view** is not attached to any decision. It is an exploration
  surface, and putting a risk analysis inside one means it is seen when someone
  goes looking, which is not when it matters.

`src/core/envelope.rs` already carries the chain this rests on: "every spawn is
approved individually → the preview must be reviewable → the preview must be
short → the children must be few." Merge-order risk is exactly the kind of fact
that chain exists to make readable, and the per-call bound is what keeps adding
it from turning the preview into a form nobody reads.

## Why declared scope, and not inference

This is the part that was not obvious, and it changes the shape of the work.

`src/tools/subagent.rs::preview` receives each child as a **directive in prose**,
plus a policy and two bounds. It does not know which files a child will touch.
The graph queries that would compute the risk — `graph::blast_radius`,
`graph::between`, and the change mapping — all operate over files and symbols.
There is nothing to join them to.

That leaves two ways to get a file set out of a directive:

1. **Ask a model to infer it.** Rejected. It would make the risk analysis
   non-deterministic, which is the one property that makes it worth having: this
   is the same move as the evidence gate, replacing an assertion with a
   derivation. A model guessing which files a child will touch, and a human
   authorising a fan-out on the strength of that guess, is a worse failure than
   showing no risk at all.
2. **Have the spawner declare it.** Accepted. The child declares the paths it
   intends to work in, the overlap analysis is a deterministic function over
   those declarations plus the recorded graph, and a declaration that turns out
   to be wrong is visible later as a diff outside the declared scope.

So merge-order risk becomes: **the intersection of declared scopes, plus the
graph-reachable neighbourhood of each scope, computed between every pair of
children in the proposed draw.** Two children whose scopes are disjoint but whose
blast radii overlap are the interesting case, and the deterministic graph is what
makes that case visible.

**Undeclared scope must read as unknown.** This follows the D7 precedent: three
of six file-metadata questions carry a fourth answer —
*mjolnr could not look* — because a pair of `false`s cannot distinguish "not
binary" from "never sniffed". A preview that shows no collisions because no child
declared a scope, and a preview that shows no collisions because the scopes are
genuinely disjoint, must not render identically.

## Consequences

- **E7's remaining scope is bounded by this.** The full interactive canvas is
  what is left of E7 as a *view*; the risk analysis moves to the spawn path and
  is no longer a reason to hold E7 open.
- **`spawn_subagent` gains an optional per-child path scope.** Optional, because
  requiring it would break every existing caller and because a fan-out over work
  with no natural file boundary is legitimate. Its absence is reported, not
  defaulted away.
- **Nothing about this authorises anything.** The analysis is a fact shown before
  an approval. It does not narrow an envelope on its own, does not refuse a
  spawn, and does not merge anything. A human reads it and decides.
- **A declared scope is not a sandbox.** It is a statement of intent used to
  compute risk, not a containment boundary. `definition-of-done.md` already
  places OS-level sandboxing outside the finish line, and this must not be
  mistaken for it.

## Rejected alternatives

- **Risk in the review surface.** Arrives after the cost is sunk. Keeps its own
  job — handling a collision that actually happened — but is not the decision
  point.
- **Risk in the graph view.** Not attached to a decision; would be looked at by
  someone already exploring, which is the wrong audience at the wrong time.
- **Model-inferred file scope.** Trades the property that makes the analysis
  worth showing for the convenience of not declaring anything.
- **Refusing a spawn on detected overlap.** Rejected as authority mjolnr does not
  have. Overlapping children are sometimes exactly right — two agents on the same
  module with a serialised merge is a normal plan. mjolnr shows the risk; the
  human takes it or does not.
