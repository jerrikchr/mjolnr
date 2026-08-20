# ADR-0018: Rename `mjolnr` → `mjolnr` (Mjolnir)

**Status:** Accepted (2026-08-19)
**Date:** 2026-08-19
**Deciders:** Jerrik Christiansen
**Related:** `docs/renaming-to-mjolnr.md` (brainstorm → decision), `Cargo.toml`, `desktop/src-tauri/tauri.conf.json`, `AGENTS.md` §8 (provenance), `docs/pre-release-checklist.md`, ADR-0007 (obsidian/cyan palette), ADR-0009 (mockup layout)

## Context

`mjolnr` is Danish for "smith" — a fitting metaphor (the work is shaped, held to a standard, handed back inspectable). The product has grown past a single metaphor into a full mythic vocabulary for the runtime's own concepts:

- **Mjolnir** — forged by a dwarf, wielded by a god → model proposes, deterministic Rust disposes. Returns after every throw → runtime re-establishes control each turn. Only the worthy lift it → policy gates.
- **Yggdrasil** `src/graph` — deterministic code graph (files/symbols/calls) as world-tree; trunk = main lineage, branches = plans/worktrees (`mjolnr/ext-*`, `mjolnr/sub-*`), leaves = diffs; growth rings = durable transcript.
- **Mímir's well at the roots** — knowledge/context graph; querying is drinking to enrich the next turn (Huginn = context window flying out vs Muninn = append-only record that stays).
- **Temporal Loom** (Loki S2) — propose → approve → execute → verify → transcript (threads woven, shuttle advances, strand fused). Recovery = catching branches before they fall.
- **Norns** (Urðr/Verðandi/Skuld) — past/present/future session states (durable transcript / active plan / proposed next steps).
- **The Thing** (assembly *under Yggdrasil*) — council deliberation; models debate around the tree, `Accepted/Rejected/Deferred` as the Thing's verdict (ravens circling → converging for the verdict).
- **Bifröst** — provider streaming bridge; **Ragnarök** — hard crash/recovery.

Two constraints force the shape of the rename:

1. **Pre-release, not yet published** — `publish = false`, no crates.io artifact; migration can be graceful but must not break existing workspaces.
2. **Cheap-model execution** — the cost of renaming is kept low by doing the mechanical bulk with a low-cost model; the human reviews the small, trust-critical seams.

The binary name stays 6 characters either way (`mjolnr` → `mjolnr`; English spelling `mjolnir` is the prose name, `mjolnr` the binary).

## Decision

Rename the product from `mjolnr` to **`mjolnr`**.

1. **Binary and package:** `mjolnr` → `mjolnr` (`Cargo.toml` `package.name`, binary name, `desktop/src-tauri/tauri.conf.json` `productName`/`identifier`, scripts, CI).
2. **Workspace/config directory:** `.mjolnr/` → `.mjolnr/` as canonical. Ship a **compat shim for one release**: read `.mjolnr/` first, fall back to `.mjolnr/` when absent, and **migrate on write** (first write creates `.mjolnr/` and copies forward; never delete `.mjolnr/` automatically). Document the shim and its removal date.
3. **Code names:** keep internal `mjolnr::` crate/module/type names stable in this ADR. Add the mythic layer for UI/copy now (`Yggdrasil` canvas, `Thing` council, runic glyphs), then rename internals incrementally behind aliases — no big-bang crate rename.
4. **Docs:** update `README.md`, `AGENTS.md` title/header, `docs/` canonical references, Tauri bundle names, `THIRD_PARTY.md` product name, help strings, and `docs/pre-release-checklist.md` — then flip `docs/renaming-to-mjolnr.md` status to **Migrated**.
5. **Market/DNS:** `mjolnr`/`mjolnir` availability is TBD and non-blocking; track as a follow-up, not a gate.

## What this does not license

- No silent dropping of `.mjolnr/` workspaces — the fallback read is mandatory until the shim's advertised removal.
- No deletion of user data on upgrade, no implicit re-basing of worktrees, no widening of policy/credentials during migration — the compat shim is read-only until an explicit write.
- No third-party JS in the webview or new trust model — branding only; ADR-0006 trust classes and ADR-0016 plugin governance are unchanged.
- No claim that `mjolnr` is an OS sandbox; the policy gate remains a policy gate (AGENTS.md §3).

## Consequences

- New installs use `mjolnr` and `.mjolnr/`; existing installs keep working via the shim and migrate naturally.
- Docs, help, and bundle metadata consistently reference `mjolnr`; internal code can lag without breaking the UX contract.
- Future visual work can lean into the mythic vocabulary without renaming code first (canvas = Yggdrasil + Loom toggle, council = The Thing, recovery = Ragnarök).

## Rejected alternatives

- **Keep `mjolnr` forever.** Understandable, but the mythic mapping is already load-bearing for how we explain the graph/council/provenance, and `mjolnr` does not carry it.
- **Full code rename in one PR.** Higher risk, higher cost, no user benefit — the product is unused as a published artifact, so user-facing rename + shim dominates.
- **Two binaries (`mjolnr` + `mjolnr`) long-term.** Perpetual confusion for no benefit; one binary, one shim window.

## Migration plan (cheap-model executable)

The cheap model's checklist — mechanical, reviewable, low risk:

1. `Cargo.toml` — `name = "mjolnr"`, `[[bin]] name = "mjolnr"` (keep `mjolnr` as alias if trivial, otherwise shim binary prints `use mjolnr`).
2. `desktop/src-tauri/tauri.conf.json` — `productName`, `identifier`, bundle.
3. Workspace discovery — `src/context/mod.rs` / `src/runtime/*` path resolution: canonical `.mjolnr/`, fallback `.mjolnr/` read, migrate-on-write helper (unit-tested).
4. Docs — `README.md` (install `cargo install --path .` yields `mjolnr`, quick-start `mjolnr init`), `AGENTS.md` header, `docs/*.md` search-replace with human review, `THIRD_PARTY.md` product line.
5. `cargo fmt --all -- --check` + `cargo clippy --all-targets --all-features -- -D warnings` + `cargo test --all-features` + `cargo deny check` green; commit with `publish = false` intact.

Human reviews: shim read-fallback, Tauri bundle identifiers, and any credential/path handling — nothing else needs senior eyes.

## Revisit if

- A hosted `mjolnr` service or team registry makes the binary/workspace naming load-bearing for a protocol (then revisit crate rename timing).
- `.mjolnr/` → `.mjolnr/` shim removal becomes due — file a follow-up ADR to drop it.
