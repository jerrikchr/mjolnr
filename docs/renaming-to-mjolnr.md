# Renaming smed → mjolnr

**Status:** Partially migrated — see [ADR-0018](adr/0018-rename-smed-to-mjolnr.md).
Contract surfaces are done and guarded by `tests/branding.rs`: commands, branch
prefixes (`mjolnr/sub-*`, `mjolnr/ext-*`), the `MJOLNR_` environment prefix, the
user config directory, `.gitignore`, and the install/release scripts. The
cosmetic prose sweep (checklist item 4) is **outstanding** — see
[`rename-sweep.md`](rename-sweep.md). Flip this line to *Migrated* when that
lands, not before.
**Author:** Jerrik + agent discussion
**Date:** 2026-08-19
**Decision:** Rename `smed` → `mjolnr`. Keep internal crate/module names stable where possible and add the mythic layer for UI/copy first; migrate code names incrementally. `mjolnir` is the English spelling — `mjolnr` is the short, 6-char binary name.

---

## The core idea

Rename `smed` to **`mjolnr`** (short for **Mjolnir**, Thor's hammer). The name
is a nod to Norse mythology, Danish / Viking heritage, and the thematic fit
between a governance-core coding harness and a hammer that strikes true.

---

## Why Mjolnir fits

| Mjolnir property | smed parallel |
|---|---|
| Forged by a dwarf, wielded by a god | The model proposes; smed's deterministic code disposes. The hammer is the tool, not the wielder. |
| Returns to Thor's hand after every throw | The runtime owns every side effect and re-establishes control after each model action. Propose → approve → execute → verify → back to the loop. |
| Only those worthy can lift it | Policy gates. A model cannot self-approve, cannot widen its own scope, cannot bypass a guard. |
| Short handle (mythological flaw) | Deliberately narrow, focused tool surface. Not a general-purpose agent — a governed execution harness. |

---

## The broader Norse mythic mapping

This is where the branding gets interesting. The mythology offers a rich
vocabulary for the concepts smed already implements:

### Yggdrasil — the session / execution graph

Every completed tool call is a node; every dependency between them is a branch.
The session's execution tree *is* the world-tree, with the current path
highlighted. A knowledge graph or timeline of events rendered as Yggdrasil
would be both visually striking and conceptually exact.

### The Temporal Loom (Loki Season 2) — the plan/approval pipeline

In the climax of *Loki* Season 2, Loki destroys the failing Temporal Loom,
grabs the dying, glowing branches of time, and infuses them with his own magic.
He ascends to the End of Time, sitting on a throne holding the infinite,
branching realities together — transforming the multiverse into a living tree
resembling Yggdrasil.

Mapping:
- **Proposed tools** are threads being woven into the loom.
- **Approval** is the loom advancing a strand.
- **Execution** fuses the strand into the tree (the durable transcript).
- **Recovery** after a partial or failed execution is Loki catching the
  branches before they fall and re-weaving them.

### The Norns (Urðr, Verðandi, Skuld) — the three session states

| Norn | Meaning | smed concept |
|---|---|---|
| Urðr (Wyrd) | What has become — the past | The append-only durable transcript. Everything that actually happened. |
| Verðandi | What is becoming — the present | The active plan. Tools being approved and executed right now. |
| Skuld | What shall be — the future | Proposed next steps. What the model wants to do next, before the gate decides. |

### Bifröst — the provider / streaming connection

The rainbow bridge between Midgard and Asgard. Maps to the streaming
connection between the model provider and the runtime — the shimmering link
between two worlds, carrying tokens across.

### Huginn & Muninn (Thought & Memory)

Odin's ravens:
- **Huginn** (Thought) — flies out across the world, returns with news. Maps
  to the model's context window: it reaches out, gathers, and reports back.
- **Muninn** (Memory) — stays and remembers. Maps to the session's durable
  record, the append-only transcript.

### Mímir's well — the knowledge graph (at the roots of Yggdrasil)

Mímir's well lies at the roots of Yggdrasil. Whoever drinks from it gains
wisdom. In smed terms: the well *is* at the roots — querying the code/knowledge
graph is drinking from Mímir to enrich the next turn. Huginn (thought) vs Muninn
(memory) makes the FTS5 vs durable transcript split tangible: thought flies out,
memory stays. Canvas vision: zoom canopy → branch → leaf, colour by
`TrustClass` (`SmedGoverned` vs `ExternalUnverified` branches look different;
fresh `smed/ext-*` shoots hang until `Import` grafts them).

### The Thing (Aesir assembly under Yggdrasil) — the council

The council is deliberation, not fate-weaving — Norns (Urðr/Verðandi/Skuld) stay
as past/present/future *session states* (durable transcript / active plan /
proposed next steps). The Thing maps to council rounds: voices debating around
the tree, `Accepted/Rejected/Deferred` as the Thing's verdict. Ravens circling
→ converging at the trunk gives a natural council animation.

### Ragnarök — session crash / catastrophic rollback

The tree burns, the world ends, and then it regrows. Maps to a hard reset
after corruption, or a session recovery from a checkpoint after a crash.

---

## Practical considerations & migration (cheap-model execution)

- **`mjolnr`** is 6 characters — same length as `smed`, easy to type; `mjolnir`
  is the English spelling, `mjolnr` the binary name.
- **Binary:** `smed` → `mjolnr` (`Cargo.toml` `package.name`, `desktop/` Tauri `productName`/`identifier`).
- **Config/workspace dir:** `.smed/` → `.mjolnr/` with a **compat shim** — read `.mjolnr/` first, fall back to `.smed/` for one release, and migrate on write. This is what makes the rename cheap.
- **Keep code names stable first:** internal `smed::` crate/modules/types stay as-is; add the mythic layer for UI/copy only, then rename internals incrementally behind a feature flag / alias.
- The brand vocabulary is rich: hammer, forge, worthy, loom, weave, branch,
  tree, root, raven, well.
- The TUI could use runic-inspired glyphs for status indicators and
  branch-drawing characters for tree visualizations.
- The mythic mapping is deep enough that it can inform naming of internal
  modules, types, and UI components without feeling forced.
- **Docs pass:** update `README.md` (install/quick-start), `AGENTS.md` title,
  `docs/` canonical references, Tauri bundle names, `THIRD_PARTY.md` product
  name, and help strings — then flip this doc's status to **Migrated**.

---

## Decision

Decided 2026-08-19 — see **[ADR-0018](adr/0018-rename-smed-to-mjolnr.md)**. The
compat-shim migration plan above is the accepted execution plan, to be run
with a low-cost model while you are out. The former open questions 1–5 are
resolved as: `mjolnr` is short and Danish/Viking-rooted (like `smed`); the
theme is opt-in and carries the existing governance metaphors; migration cost
is bounded by the fallback read of `.smed/`; domain/hosting is TBD but not
blocking.

## Concept to prototype

A one-page **Yggdrasil canvas + Thing council overlay** in `desktop/`:
interactive tree (trunk = main lineage, branches = plans/worktrees, leaves =
file diffs) with a Temporal Loom toggle for the approval pipeline (threads
woven → shuttle advances → strand fused into the tree). Recovery is Loki
catching the branches before they fall.

## Handoff for the cheap model (while Jerrik is at lunch)

**Goal:** execute ADR-0018 mechanically so the rename ships as `mjolnr` without
breaking existing `.smed/` workspaces.

**Checklist (in order, one commit):**

1. `Cargo.toml` — `package.name = "mjolnr"`, `[[bin]] mjolnr` (keep `cargo run --bin smed` alias only if one line; otherwise a shim that prints `use mjolnr`).
2. `desktop/src-tauri/tauri.conf.json` — `productName`/`identifier`/bundle strings.
3. **Compat shim** — `src/context/mod.rs` and any workspace path helper: canonical
   `.mjolnr/`, fallback read from `.smed/` when `.mjolnr/` absent; migrate on
   first write (create `.mjolnr/`, copy forward, never delete `.smed/`). Add a
   unit test for the fallback.
4. **Docs sweep** — `README.md` (install yields `mjolnr`, `mjolnr init` quick-start),
   `AGENTS.md` header, `docs/*.md` canonical references, `THIRD_PARTY.md` product
   name, help strings. Update this doc's status line to **Migrated** at the end.
5. **Gate:** `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
   `cargo test --all-features`, `cargo deny check` — all green, `publish = false` intact.

**Human reviews only:** the shim's read-fallback and Tauri bundle IDs. Everything
else is mechanical and safe for a cheap model to land directly on `main` or a
small PR, whichever you prefer.

**Prompt to paste to the cheap model:**

> Execute `docs/adr/0018-rename-smed-to-mjolnr.md` verbatim. Keep internal
> `smed::` module names for now — only binary/package/workspace-dir/docs move.
> Follow `docs/renaming-to-mjolnr.md` "Handoff for the cheap model" checklist.
> One commit, gates green, no deletion of `.smed/` data.
