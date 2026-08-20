# ADR-0007: Obsidian/Paper Cyan token palette for the D-phase surfaces

**Status:** Accepted
**Date:** 2026-07-30
**Deciders:** Jerrik Christiansen
**Phase:** D5 (applies to every remaining D-phase surface)

## Context

`docs/integrated-workspace-phases.md` §3.7 fixes the D0–D12 visual language:

> D0–D12 use the checked-in shadcn-svelte Nova/Zinc system, Cyan charts,
> HugeIcons, Geist Mono, small radius, default/translucent menus, and
> system-following light/dark themes. No phase invents a second component
> library or custom theme.

A design exploration captured in `mockups/index.html` produced a full token
system accepted as the direction: **Obsidian Cyan** (dark)
and **Paper Cyan** (light), Inter for prose, JetBrains Mono for code, and its own
4→16 px radius scale. D5's source-control surface is the first phase surface that
has to choose, so the conflict is decided now rather than discovered twice.

Two facts narrow this considerably:

1. **The component library is not in dispute.** The mockup is styling, not a
   second primitive set. `desktop/src/lib/components/ui/` — the generated
   shadcn-svelte library that ADR-0005's supersession note names as current
   implementation truth — stays exactly as it is. §3.7's load-bearing sentence,
   "No phase invents a second component library", is satisfied.
2. **The mockup's governance colours already implement the committed
   contract.** `docs/tauri-design-system.md` §42–43 requires a distinct visual
   class for *proposed/live, attention/approval, verified, refused/failed,
   uncertain/recovery*. The mockup's `--gov-proposal`, `--gov-approval`,
   `--gov-verified`, `--gov-refusal`, and `--gov-uncertain` map onto those five
   one for one, in both themes. This is the first artefact in the project that
   supplies that vocabulary as tokens rather than as a requirement.

So the actual disagreement with §3.7 is three items: mono font, palette, and
radius scale.

## Decision

Adopt the mockup's token system for the remaining D-phase surfaces, and amend
§3.7 to match. Specifically:

- Palette: Obsidian Cyan (dark) / Paper Cyan (light), as semantic CSS custom
  properties. **Both themes are required**, not dark-only — D1's acceptance
  already demands that light and dark system themes pass desktop tests, and the
  mockup carries a real `[data-theme="light"]` palette rather than an inversion.
- Typography: Inter for prose, JetBrains Mono for code and every revision,
  hash, path, and diff line. Replaces Geist Mono.
- Radius: the mockup's `--r-xs` … `--r-pill` scale replaces "small radius".
- Governance colour is a **token, never a literal**. A surface that needs to say
  "refused" uses `--gov-refusal`; it does not pick a red.
- The generated shadcn-svelte component library, HugeIcons, and Cyan charts are
  unchanged.

## What this decision does not license

The mockup is a visual reference, not a behavioural one. Two of its cells state
things the runtime cannot support, and the surfaces built from it must not
reproduce either:

1. **`sync: synced`, rendered in the verified colour.** `tauri-design-system.md`
   is explicit: "No client component claims applied, approved, verified, or
   recovered state." Painting a remote-sync value in `--gov-verified` is that
   claim. See ADR-0008 for the honest computation this is replaced by; the short
   version is that ahead/behind is knowable locally but only *as of the last
   fetch*, and the qualifier is not optional.
2. **A repository panel with no capture marker.** The mockup reads `branch main /
   head a1b2c3d / dirty 0` as though that is the repository now. Nothing watches
   the filesystem — `RepositoryFreshness` deliberately has no `fresh` variant —
   so every surface rendering a projection also renders when it was captured.

A visual reference cannot authorise a state claim. Where the mockup and
`AGENTS.md` §1.3 disagree, the mockup loses, and the report says which cell
was changed and why.

## Alternatives rejected

**Build D5 against §3.7 as written, re-skin later.** Rejected: it guarantees a
rewrite of every surface built in the interim, and the owner has already accepted
the replacement direction. Building to a contract nobody intends to keep is how
a design system ends up with two of everything.

**Adopt the mockup wholesale, including its cells.** Rejected for the reasons in
the section above. The palette is a decision about how mjolnr looks; `synced` in
green is a decision about what mjolnr claims, and those are not the same kind of
decision.

**Dark-only, matching the mockup's default.** Rejected: D1's acceptance requires
both themes, D12 requires an accessibility and light/dark review, and the mockup
already has the light palette — there is nothing to trade away.

## Accepted costs

- §3.7 of `integrated-workspace-phases.md` is amended, and the amendment is
  itself a documented act rather than a silent divergence.
- Two font families are now named where one was. Both must be checked in or
  vendored, not fetched at runtime — a packaged desktop app that reaches a font
  CDN on launch is a network dependency nobody declared.
- Surfaces already built (Conversation, Plan, Changes, Verify, Attention) use the
  previous tokens. They are not retro-fitted by this ADR; whichever phase
  touches them next moves them, and D12's integration checkpoint is where a
  mixed system would be caught.

**Update, E2.5 (2026-07-31): palette and radius landed; type scale did not.**
`desktop/src/app.css` carries the Obsidian Cyan / Paper Cyan colour values
described above — dark in `.dark{}`, light in `:root{}` (the mockup's own
file organises it the other way round; the app keeps its existing
`.dark`-class theme mechanism rather than switching to the mockup's
`data-theme` attribute, same palette either way) — and owner review of the
running app confirmed the colours read correctly. Inter and JetBrains Mono
are real self-hosted `@fontsource-variable` packages, replacing Geist Mono.
The radius scale and the `--gov-*`/`--accent-cyan` tokens (the latter kept
distinct from shadcn's own `--accent`, per the naming collision noted above)
are also in place and correct.

**Not landed: the `--text-xs`…`--text-xl` (11–20px) type scale.** The
variables exist in `app.css` but nothing consumes them — every shadcn
primitive still renders at Tailwind's own default type scale, which reads
visibly larger and more spacious than the mockup at every density this ADR
was meant to cover. Owner review caught this directly: colours matched, but
the interface did not read as the mockup's dense instrument panel. This is
open work, tracked in `docs/`'s reopened §E2.5, not a
retroactive edit to what this ADR decided — the decision (adopt the
mockup's scale) stands; only the application of it is incomplete. Same
caveat for Conversation/Plan/Changes/Verify/Attention: their governance-token
*colours* landed in the same slice, but their control sizing/spacing did
not move off the shadcn defaults either.

Type-scale/density is only part of what owner review found missing — a
second, still-unenumerated gap of mockup elements the five stages never
built at all also stands. `docs/` §E2.5's status block is
the current source of truth on both gaps; this work is parked pending a
full element-by-element audit, not in progress.
