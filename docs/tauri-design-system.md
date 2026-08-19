# smed Tauri design system

Status: **Approved contract; implementation belongs to Phase A0**

Date: 2026-07-28

## Purpose

The Tauri workspace needs a coherent system before it needs many screens. This
document governs the desktop client's visual language, interaction primitives,
accessibility, and component quality. It prevents the Orca-like workspace from
becoming a collection of locally styled panels.

The Ratatui and Tauri clients share smed's semantic vocabulary, not rendering
code. [`docs/tui-design.md`](./tui-design.md) remains authoritative for the
terminal.

## Product character

smed should feel calm, capable, direct, and trustworthy. Density is useful
when hierarchy remains obvious. Chrome recedes; work, attention, evidence, and
human decisions lead.

“Modelled after Orca” means the workspace hierarchy, focus, restraint, and
onboarding quality are reference points. It does not permit copying source,
CSS, exact components, text, or interaction implementations.

## Visual Identity & Emblem

The primary visual emblem for the desktop client is a **single gate bar**. It symbolises the boundary that every effect must clear without implying that smed provides security beyond its explicit policy gate. The mark is deliberately readable at 16px, where anything more is noise.

## Token layers

All component styling consumes semantic CSS custom properties. Components must
not contain unexplained literal colours, spacing, radii, shadows, typography,
or animation timings.

1. **Foundation:** neutral ramps, font families, type scale, spacing scale,
   radii, border widths, elevation, motion duration, and easing.
2. **Surface:** canvas, workspace, raised, inset, hover, selected, scrim.
3. **Content:** primary, secondary, muted, disabled, inverse, link.
4. **Governance:** proposed/live, attention/approval, verified, refused/failed,
   uncertain/recovery.
5. **Focus:** keyboard focus ring and selected-with-focus treatment.

Governance colour always has a second cue: icon, label, shape, or position.
Colour alone may never communicate authority or outcome.

## Core primitives

Phase A0 must implement and exhibit the following. Seed the required accessible
behavior from selected shadcn-svelte/Bits UI components where it is a fit, then
adapt their source to this contract:

- `Button`, `IconButton`, and destructive/approval variants;
- `TextField`, `TextArea`, `Select`, and validation/help text;
- `Tabs` and segmented controls;
- `Dialog`, `Popover`, `Menu`, and `Tooltip`;
- `Toast` or inline notice for transient feedback;
- `Badge` and `StatusMark` for governed state;
- `Card`, `ListRow`, `Divider`, `ScrollArea`, and empty/loading/error states;
- `SplitPane` and resizable workspace regions;
- `CommandPalette`;
- typography and icon primitives.

Product-specific compositions such as work rail, transcript item, approval
request, recovery decision, plan revision, change group, and verification
evidence are built from these primitives after the gallery is accepted.

## Interaction rules

- Keyboard access is complete, not supplementary.
- Focus is always visible and returns predictably when an overlay closes.
- Escape closes the topmost reversible overlay; it never resolves an approval.
- Destructive and governed actions state their consequence in text.
- Hover-only affordances have keyboard and touchpad-accessible equivalents.
- Loading, empty, unavailable, refused, uncertain, and stale states are
  distinct.
- Motion explains continuity or change. It never delays work or substitutes
  for state.
- Reduced-motion preference removes non-essential movement.
- Dense and comfortable display modes use the same primitives and semantics.

## Accessibility floor

- Target WCAG 2.2 AA contrast and interaction behavior.
- Use native HTML semantics before ARIA.
- Every input has a programmatic label and useful error association.
- Dialogs trap focus and restore it.
- Menus, tabs, listboxes, and command results follow their expected keyboard
  patterns.
- Pointer targets remain usable at desktop scale.
- At 200% zoom and narrow window widths, governed actions and status text
  remain available without clipping.

## Component gallery

The desktop client ships an internal development route or window that displays
every primitive and governed state. It is the review surface for:

- light and dark themes if both are supported;
- normal, hover, focus, active, disabled, loading, and error states;
- proposed, attention, verified, refused, and uncertain governance;
- long labels, localization expansion, and truncated content;
- compact and comfortable density;
- reduced motion and high-contrast operating-system preferences.

Do not require Storybook merely because it is conventional. The gallery is a
SvelteKit client route using the same build and runtime as the Tauri client
unless a later dependency decision demonstrates material value.

## Performance rules

- A state update should invalidate only components that consume the changed
  signal.
- Virtualize long work lists and transcripts when measurement proves it is
  needed; do not pre-emptively virtualize short collections.
- Avoid layout measurement loops and animation of layout-heavy properties.
- Canvas libraries cannot render ordinary workspace chrome.
- Record production bundle size and a representative high-frequency update
  profile at every desktop phase checkpoint.

## Governance

Adding a primitive requires:

1. a demonstrated recurring need;
2. keyboard and accessibility behavior;
3. all semantic states in the gallery;
4. tests at the behavior boundary;
5. a design-system update in the same commit.

Prefer extending or deleting an existing primitive over introducing a near
duplicate. No product surface may create a private status vocabulary. Bringing
in shadcn-svelte source is an explicit addition: record provenance, review the
generated dependencies and styles, delete unused variants, and add smed-owned
behavior tests.

**`components.json`'s `baseColor: "zinc"` is generator metadata, not the live
palette** (ADR-0007). The shadcn-svelte schema has no "Obsidian Cyan" option,
so this field stays `zinc` even though `desktop/src/app.css` overrides every
value it seeds. Running `npx shadcn-svelte add <component>` for a *new*
primitive writes zinc-default token values for anything not already covered
by an existing `--color-*` mapping in `app.css` — check the diff after
generating and fold any new variable into the existing remap rather than
letting it stand.

## Phase A0 exit criteria

- Token layers and documented naming exist.
- Every core primitive renders in the gallery.
- Keyboard navigation and focus restoration pass automated and manual checks.
- Governance states use text or shape in addition to colour.
- Reduced motion and narrow/zoomed layouts have evidence.
- No client component claims applied, approved, verified, or recovered state
  without the corresponding runtime DTO.
- A designer review and an accessibility review are recorded in the phase
  report.
