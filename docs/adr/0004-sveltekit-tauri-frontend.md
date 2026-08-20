# ADR 0004: SvelteKit for the Tauri frontend

- Status: Accepted
- Date: 2026-07-28
- Deciders: Product owner and implementation planning review

## Context

mjolnr is adding a Tauri rich workspace over its Rust runtime. The client must
render streaming activity, changing work state, approvals, recovery, plans,
changes, evidence, onboarding, settings, and a reusable design system.

Tauri is frontend-agnostic and acts as a static web host. React was initially
proposed because it is familiar and has an official Konva binding. The future
node canvas is additive, however, and Konva also has a framework-independent
API. A deferred canvas must not choose today's application architecture.

SolidJS was then considered for its fine-grained reactivity. Svelte 5's
reactivity primitives also compile reactive dependencies into targeted update
effects. Synthetic DOM
benchmarks therefore do not justify choosing the smaller Solid ecosystem for
mjolnr. Accessibility, component ergonomics, tooling, and team maintainability
are part of performance too.

## Decision

Use **Svelte 5 + SvelteKit + TypeScript** for the Tauri frontend.

Configure SvelteKit exactly as a Tauri-hosted static SPA:

- `@sveltejs/adapter-static`;
- `fallback: "index.html"`;
- root `ssr = false`;
- Tauri `frontendDist` pointing at SvelteKit's `build/` output;
- no SvelteKit server routes, server hooks, remote functions, or backend
  authority.

SvelteKit earns its additional layer through the official Svelte router,
layouts, error boundaries, and predictable organization for onboarding,
workspace, settings, and the component gallery. Routes are reversible client
presentation state. They do not identify authoritative runtime state or grant
authority.

Use **shadcn-svelte** as the source-owned starting point for accessible
primitives. Bring in only components mjolnr needs. shadcn-svelte is not mjolnr's
design system: selected source is adapted to mjolnr's semantic tokens,
interaction rules, and component API, then maintained in the repository.
Underlying Bits UI and Tailwind dependencies require the same justification,
licence record, and update discipline as any other dependency.

Svelte is justified by current needs:

- Svelte 5's reactivity primitives provide granular reactive updates for streaming DTO state;
- the compiler performs useful accessibility checks while the design system is
  being built;
- HTML/CSS-shaped components lower the maintenance burden for a mixed team;
- SvelteKit provides the official router and a documented Tauri static-SPA
  configuration;
- shadcn-svelte provides editable, source-owned primitives rather than a
  visually prescriptive black-box component library.

Do not select a canvas library in this decision. A future canvas must live
behind a client-local adapter. Raw Konva remains one candidate.

## Alternatives

### React

Rejected as the default. Its ecosystem and hiring familiarity are strong, but
mjolnr does not need React to use Tauri or Konva. Choosing it for a deferred
binding would be speculative coupling.

### SolidJS

Rejected as the default after reconsideration. Its fine-grained signal model is
excellent, but Svelte 5 also performs granular reactive updates. Solid's narrow
rendering advantage is not decisive enough to outweigh Svelte's compiler
tooling, accessibility warnings, component ecosystem, and maintainability.

### Svelte without SvelteKit

Credible fallback and slightly smaller in conceptual scope. Rejected because
mjolnr already has several durable client destinations and needs routing,
layouts, error boundaries, and an internal design-system gallery. Adding those
piecemeal would recreate a subset of SvelteKit.

### Vanilla TypeScript

Rejected for the product frontend. It may minimize framework overhead in a
small benchmark, but mjolnr already needs reactive state, component composition,
lifecycle, accessible overlay primitives, and a maintained design system.
Owning those mechanisms would cost more complexity than the framework removes.

### Rust-to-Wasm frontend frameworks

Rejected for now. Keeping frontend rendering in Rust would reduce language
count but would narrow the desktop UI and accessibility ecosystem and add
Wasm/webview boundary complexity without strengthening mjolnr's Rust authority.

## Consequences

- Svelte, SvelteKit, the static adapter, shadcn-svelte-derived source, Bits UI,
  and Tailwind are evaluated and recorded in `THIRD_PARTY.md` when Phase A0
  introduces them.
- The design system uses semantic CSS variables and Svelte components but keeps
  its vocabulary independent of the framework.
- Runtime/client DTOs remain framework-neutral.
- The Phase A0 report must include a production bundle report and representative
  localized-update profile.
- If the measured A0 workload shows material regression against a bounded
  vanilla reference, implementation stops and this ADR is reconsidered.
- React-specific libraries and patterns do not enter the frontend by default.
- SvelteKit browser history and routes remain client navigation only. Runtime
  IDs and snapshots remain the source of truth.
- Copied component source must retain required notices and receive mjolnr-owned
  tests; upstream appearance is not inherited as product direction.

## References

- Tauri frontend configuration:
  <https://v2.tauri.app/start/frontend/>
- Tauri SvelteKit configuration:
  <https://v2.tauri.app/start/frontend/sveltekit/>
- Svelte compiler and framework overview:
  <https://svelte.dev/>
- Svelte compiler warnings:
  <https://svelte.dev/docs/svelte/compiler-warnings>
- shadcn-svelte:
  <https://www.shadcn-svelte.com/docs>
- Konva framework-independent documentation:
  <https://konvajs.org/docs/>
