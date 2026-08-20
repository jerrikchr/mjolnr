# ADR-0009: The design-overhaul mockup's layout is the structural target for D2, D5, D7, and D8

**Status:** Accepted
**Date:** 2026-07-30
**Deciders:** Jerrik Christiansen
**Phase:** D2, D5 (UI), D7, D8

## Context

ADR-0007 accepted `mockups/index.html` for exactly three things: palette,
typography, and
radius. It was explicit that "the component library is not in dispute" and
said nothing about layout, because at the time the mockup's scope genuinely
was tokens.

The mockup later grew past that boundary and added structure that ADR-0007
never ruled on:

- a sidebar with **Worktrees** and **Fleet** (subagent) sections showing
  status orbs, branch tags, and model tags per running child — the target for
  D2's UI;
- a **file explorer** with folder/file nodes, modified/staged markers, and a
  **Repository** panel at its foot showing branch/head/dirty/staged/sync —
  the target for D5's UI;
- a two-pane canvas (`pane-conversation` / `pane-editor`) with file tabs and a
  trust label (`mjolnr-governed · read-only`) — the target for D7's editor surface;
- a bottom terminal pane with tabs, status orbs per process, and an
  `operator-controlled` label — the target for D8's terminal surface.

Two sections in the mockup do **not** correspond to any D-phase and should not
be read as one:

- **Persona** and **Council** (the governance modal's Multi-Model Council and
  Model & Role Routes tabs) are existing governance concepts, not new D-phase
  scope.
- **Accounts**, as drawn, lists Anthropic/OpenAI/Ollama with status orbs —
  these are model-provider accounts, an existing concept distinct from D6's
  GitHub/Linear task-source accounts. **D6 has no representation in the
  mockup at all** — no task list, no PR/issue surface, no GitHub or Linear
  reference anywhere in `index.html`. This is evidence, not an oversight to
  fix: D6 remains the phase with zero design certainty from this artefact.

Leaving this unstated means D2/D5/D7/D8 implementers either re-derive a layout
from scratch or guess whether the mockup binds them, which is the same kind of
two-systems risk ADR-0007 was written to prevent for tokens.

## Decision

The mockup's structural layout is now also accepted, for these four phases
specifically:

- **D2** builds its worktree/child-run UI as the sidebar's Worktrees and Fleet
  sections: status orb, name, branch tag per worktree; status orb, directive,
  model tag per running child.
- **D5**'s UI builds the file explorer's Repository panel: branch, head,
  dirty count, staged count, sync row. ADR-0007 and ADR-0008 already govern
  two cells this panel must render differently from the mockup's literal
  markup — see "What this does not license" below.
- **D7** builds the two-pane canvas: conversation pane unchanged, editor pane
  with tabs, modified-dot, close button, trust label, and a diff-meta bar
  above the code area.
- **D8** builds the bottom pane: tabbed terminals, per-tab status orb,
  `operator-controlled` label, expand/collapse and toggle controls.

This is layout and interaction structure — component boundaries, what data
each panel shows, where it sits — not a component-library change. ADR-0005's
supersession note and ADR-0007's point 1 both still hold: the generated
shadcn-svelte library is unchanged, and no phase invents a second one.

**D6 is explicitly not covered by this ADR.** No mockup section speaks to it.
Its UI remains undesigned, and the schedule risk that carries is real —
credentials, untrusted issue/PR text, two providers, two surfaces — not
reduced by anything decided here.

## What this does not license

Same caution as ADR-0007: the mockup is a visual reference, and cells that
assert a state the runtime cannot honestly claim do not become acceptable by
virtue of also being layout.

- The explorer's Repository panel reads `sync: synced` in the verified
  colour. ADR-0008 already replaces this: D5's UI renders the qualified
  ahead/behind/`remote_sync_as_of` value, never a bare `synced`, never
  `--gov-verified`.
- The panel shows `branch main / head a1b2c3d / dirty 0` with no capture
  marker, as if that were the repository right now. `RepositoryFreshness` has
  no `fresh` variant on purpose — D5's UI renders when the projection was
  captured, same as every other repository-state surface.
- The Fleet section's model tags (`o3-mini`, `haiku`) and the file explorer's
  editable-looking file tree are illustrative content, not a claim about
  which providers or files exist. D2 and D7 populate these from runtime
  truth, not from the mockup's sample data.

## Alternatives rejected

**Wait for a D2/D5/D7/D8-specific mockup pass before deciding.** Rejected:
the structure already exists and is stable enough to build against; waiting
adds a review cycle for no new information.

**Treat the whole mockup, including Accounts/Persona/Council, as phase
scope.** Rejected: Persona and Council are existing surfaces, not new work,
and folding them into a phase would misattribute already-built governance UI
to D2/D5/D7/D8's acceptance criteria.

## Accepted costs

- Four phases now carry a layout obligation they didn't have written down
  before. A report that ships a materially different layout must say
  why, the same discipline ADR-0007 imposed on token choices.
- D6 continuing to have no visual reference is now an explicit, named gap
  rather than an implicit one — see `integrated-workspace-phases.md`'s
  recommended execution order.

**Update, E2.5 (2026-07-31): the section boundaries landed; density did not
— reopened.** D2's sidebar UI shipped with the mockup's section
*structure* — status orb, name, branch tag per worktree; status orb,
activity, model-tag-shaped badge per running child — sourced from real
`ClientWorktree`/Fleet event data rather than the mockup's illustrative
`o3-mini`/`haiku` sample tags (per "What this does not license" above). The
governance modal (Persona/Council/Routes tabs) also shipped, since ADR-0009
already named those as existing-not-new scope, with its Council tab
correctly showing real Fleet activity or an honest empty state rather than
the mockup's animated node diagram or a fabricated quorum badge (see
`docs/` §E2.5 for that reasoning, which stands).

What did not land: the mockup's actual **layout structure means dense**, and
owner review of the running app found the shipped sidebar reads as
default-density shadcn components with the right sections in the right
order, not the mockup's tight instrument-panel rows. This ADR's "structural
target" was read too literally as "the right boxes in the right places" and
not literally enough as "the mockup's literal spacing and sizing govern
these boxes" — that correction is now explicit in the reopened §E2.5.
D5's file explorer, D7's editor pane, and D8's terminal pane remain
unbuilt, exactly as this ADR anticipated — E2.5 deliberately did not draw
placeholders for them, and that boundary is unaffected by the density gap.

Density is only part of what the owner's review found missing — a second,
still-unenumerated gap of mockup elements the five stages never built at all
also stands. `docs/` §E2.5's status block is the current
source of truth on both gaps; this work is parked pending a full
element-by-element audit, not in progress.
