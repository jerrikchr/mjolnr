# ADR-0005: Custom primitives over shadcn-svelte seeding

**Status:** Superseded for current desktop implementation
**Date:** 2026-07-28
**Superseded:** 2026-07-29
**Deciders:** Jerrik Christiansen  
**Phase:** A0

## Supersession note

This ADR remains the historical record of the Phase A0 bespoke-primitive
decision. The later desktop workspace standardized on generated
shadcn-svelte components with Tailwind, Nova style, Zinc base colours,
HugeIcons, Geist Mono, and small radius. The checked-in
`desktop/components.json`, `desktop/package.json`, and
`desktop/src/lib/components/ui/` are the current implementation truth.

Do not use the Phase A0 bundle comparison below to remove the current component
system. Future component additions should use the existing generated library
where it fits and record any new dependency or provenance obligations.

## Context

ADR-0004 specified:

> Use **shadcn-svelte** as the source-owned starting point for accessible
> primitives. Bring in only components smed needs.

The design-system contract (`docs/tauri-design-system.md`) further states:

> Seed the required accessible behavior from selected shadcn-svelte/Bits UI
> components where it is a fit, then adapt their source to this contract.

During Phase A0 implementation, every core primitive in the design-system
inventory was evaluated against shadcn-svelte's source. The evaluation
concluded that seeding from shadcn-svelte would introduce more adaptation
cost than writing bespoke primitives for these reasons:

1. **shadcn-svelte requires Tailwind CSS.** smed's design system uses
   semantic CSS custom properties (tokens), not utility classes. Adapting
   shadcn-svelte source would require stripping Tailwind and rebuilding all
   styling against the token contract — effectively rewriting the component.

2. **Bits UI (shadcn-svelte's runtime dependency) adds ~40 kB to the
   frontend bundle.** The entire Phase A0 client output is ~110 kB
   uncompressed. Bits UI would nearly double it for primitives that smed
   already implements with correct ARIA attributes and keyboard handling.

3. **smed's primitives have governance-specific semantics.** `StatusMark`,
   `CommandPalette`, and approval/destructive `Button` variants carry
   meaning that shadcn-svelte components do not model. The adaptation
   surface is larger than the shared surface.

4. **Provenance complexity.** AGENTS.md §8 requires that every incorporated
   dependency's licence be recorded and complied with. Seeding from
   shadcn-svelte source (MIT) is permissible, but the act of adapting it
   creates a provenance obligation that pure original code avoids.

## Decision

Phase A0 implements all core primitives from scratch, guided by the same
ARIA authoring practices and keyboard patterns that shadcn-svelte and
Bits UI follow (W3C ARIA Authoring Practices, MDN ARIA references), but
without incorporating shadcn-svelte or Bits UI source or runtime.

The design-system contract's accessibility and interaction requirements are
met by the bespoke implementations, as verified by the component gallery,
automated keyboard-navigation tests, and ARIA attribute assertions.

## Tauri desktop transitives

The desktop `deny.toml` (`desktop/src-tauri/deny.toml`) accepts the following transitives from Tauri 2:

- **MPL-2.0 licence**: Added to the licence allow-list for `webkit2gtk`, `option-ext`, and other Tauri desktop transitives. MPL-2.0 is a file-level weak copyleft that does not restrict smed's core licensing or impose copyleft on surrounding code.
- **17 unmaintained advisories**: All are transitive, unmaintained-only, and have no safe upgrade path within the Tauri 2 release series (e.g. `urlpattern`, `tauri-utils`, `glib-macros`, `wry`, `syntect` transitive chains). Each is ignored individually by RUSTSEC id with a dated reason in `desktop/src-tauri/deny.toml`.
- **Wildcard policy**: `wildcards = "deny"` is in force globally. The single carve-out is cargo-deny's `allow-wildcard-paths = true`, which permits a wildcard version requirement **only on path dependencies**. Today the sole path dependency is the local governance core (`smed = { path = "../../" }`), which always appears as a wildcard while referenced by path. A future registry wildcard dependency is refused mechanically; a future path wildcard means adding a path dependency at all, which requires the AGENTS.md §8 justification and an update to this section.

These exceptions are recorded here because the desktop crate depends on `smed` as a local path reference, and Tauri's transitive dependency graph necessarily pulls in these licences and unmaintained crates. Before adding any new Tauri transitive exceptions or wildcard allowances, update this section and ensure the desktop `deny.toml` reflects the change.

## Historical Phase A0 consequences

- **At the Phase A0 checkpoint there was no shadcn-svelte or Bits UI
  dependency** in `desktop/package.json`; the bundle was ~110 kB uncompressed.
- **At the Phase A0 checkpoint there was no Tailwind CSS dependency.**
  Styling used semantic CSS custom properties.
- **At the Phase A0 checkpoint smed owned all primitive source.** The later
  shadcn standardization superseded this condition.
- **Accessibility behavior must be tested independently** rather than
  inherited from Bits UI. The Phase A0 test suite includes focus-trap,
  keyboard navigation, and ARIA attribute tests for this reason.
- **Future primitives** may still evaluate shadcn-svelte or Bits UI on a
  per-component basis. This ADR does not foreclose that — it records the
  Phase A0 decision and its rationale.

## Alternatives rejected

| Alternative | Why rejected |
|---|---|
| Seed from shadcn-svelte, strip Tailwind | Rewrite cost ≥ fresh implementation; provenance overhead |
| Add Bits UI as a runtime dependency | +40 kB bundle for primitives already implemented; governance semantics not modelled |
| Use Tailwind to match shadcn-svelte | Conflicts with semantic-token design contract; adds build complexity |
