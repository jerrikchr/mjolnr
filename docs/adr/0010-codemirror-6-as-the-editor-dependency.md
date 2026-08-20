# ADR-0010: CodeMirror 6 is the D7 editor dependency

**Status:** Accepted
**Date:** 2026-07-30
**Deciders:** Jerrik Christiansen
**Phase:** D7

## Context

`docs/integrated-workspace-phases.md` §D7 requires the editor dependency to be
chosen in **its own recorded checkpoint, before the editor is added** —
"Evaluate CodeMirror 6 and Monaco in a recorded checkpoint before adding an
editor dependency. Compare bundle size, language support, accessibility, worker
model, removal cost, and Tauri compatibility." AGENTS.md §8 separately requires
every new dependency to carry purpose, licence, rejected alternatives, and
removal cost in the report.

The sequencing is the point. A comparison written after the editor is wired in
is a justification, not a decision, and this ADR exists so that the code that
follows cannot be the thing that settled the question.

Both candidates are MIT and both are viable. The measurements below come from
two throwaway Vite 6.4.3 production builds, built against the same Vite version
`desktop/` already pins, each mounting one editor with Rust, JavaScript, and
JSON:

| | CodeMirror 6 | Monaco (syntax only) | Monaco (+ JSON/TS services) |
|---|---|---|---|
| `monaco-editor` / `codemirror` version | 6.0.2 | 0.56.0 | 0.56.0 |
| JS + CSS, gzipped | **188 KiB** | 772 KiB | 2.68 MiB |
| Largest single chunk (raw) | 574 KiB | 2.67 MiB | 7.04 MiB (`ts.worker`) |
| Emitted assets | 1 JS | 3 JS + 1 CSS | 6 JS + 2 CSS + 1 font |
| Web workers required | **0** | 1 | 3 |
| `node_modules` footprint | 4.5 MiB | 101 MiB | 101 MiB |
| Built-in language definitions | 19 official + 103 legacy modes | 83 | 83 |

The "syntax only" Monaco column is the fair like-for-like against CodeMirror's
`basicSetup`: neither runs a language service. The third column is what Monaco
costs the moment anyone asks for the JSON or TypeScript intelligence that is
most of the reason to reach for Monaco at all.

Two things were checked rather than assumed, because both are CSP-relevant and
both are commonly asserted wrongly:

- **Neither built bundle calls `eval` or `new Function`.** The one `eval(` hit
  in Monaco's `ts.worker` is the text `declare function eval(x: string): any;`
  inside a bundled `lib.d.ts`, not a call site. mjolnr's Tauri CSP does not need
  `'unsafe-eval'` for either candidate.
- **Vite emits Monaco's workers as same-origin files**, not `blob:` URLs
  (`new Worker("/assets/editor.worker-….js")`). mjolnr's CSP declares no
  `worker-src` or `child-src`, so workers inherit `default-src 'self'`, which
  those URLs satisfy.

On accessibility the comparison goes the other way, and it is the one dimension
where Monaco is clearly ahead. Counting the affordances present in each built
bundle: Monaco carries a dedicated screen-reader layer — `accessibilitySupport`
(23 occurrences), `screenReader` (16), an `accessibilityHelp` widget, 31
`aria-label`s and 3 `aria-live` regions. CodeMirror is a `contenteditable`
surface with 10 `aria-label`s, one `aria-live` region, and
`aria-activedescendant` on its completion list. Monaco's accessibility story is
the more complete one, and choosing CodeMirror gives that up.

## Decision

**D7 uses CodeMirror 6.**

The deciding argument is not the byte count on its own; it is what the byte
count is buying. mjolnr's editor pane is one pane of a governed workspace, and
§D7 scopes it to tabs, go-to-file, find, syntax highlighting, diagnostics
display, autosave preference, and explicit save. Every one of those is
CodeMirror's `basicSetup` plus a language package. Monaco's additional 584 KiB
gzipped — before any language service — buys an IDE feature set D7 does not ask
for, and its worker-per-language-service model buys a second execution context
whose lifecycle mjolnr would then own alongside the PTYs D8 is about to add.

Removal cost decides the near tie that remains. CodeMirror is a set of small
packages assembled behind one mount function: removing it deletes that function
and the packages, and nothing else in `desktop/` learns its names. Monaco
requires a `self.MonacoEnvironment` global, per-worker entry points, a CSS
import, and a font asset — a removal touches the app's global scope, its build
configuration, and its CSP posture. Under AGENTS.md §2.2's extraction test, the
smaller blast radius wins.

`@codemirror/legacy-modes` (103 CodeMirror 5 modes, syntax highlighting only) is
accepted as the fallback for languages with no Lezer grammar. Monaco's 83
built-in definitions are a genuine advantage over CodeMirror's 19 official
packages; the legacy collection closes most of that gap at a lower fidelity that
a highlight-only pane can afford.

## What this does not license

- **This is not a decision to render a language service.** D7 ships syntax
  highlighting and *displays* diagnostics; it does not run a type checker in the
  frontend. Diagnostics arrive from Rust, from mjolnr's own governed verification
  commands, and a future frontend language service is a separate §8 checkpoint,
  not an implementation detail of this one.
- **This is not a component-library change.** ADR-0005's supersession note and
  ADR-0007 point 1 still hold: the generated shadcn-svelte library is unchanged,
  and the editor is a leaf inside it, not a second system.
- **The editor holds no authority.** A CodeMirror document is presentation
  state. Containment, staleness, and every write remain Rust's, per AGENTS.md
  §2.1 — the frontend may not decide that a save is safe.

## Alternatives rejected

**Monaco.** Rejected on the four dimensions above — 4× the gzipped weight for
the feature set D7 actually specifies, 22× the `node_modules` footprint, a
worker model mjolnr would have to own, and a removal that reaches into global
scope and build configuration. Its accessibility layer is the real loss and is
recorded as an accepted cost below, not waved away.

**Neither — ship a read-only, syntax-highlighted viewer with no editor.**
Rejected: §D7's acceptance requires a stale-on-disk save decision and
keyboard-only edit/find/save/close, none of which a viewer can satisfy. It
would also leave mjolnr claiming an editor pane in the ADR-0009 layout that
cannot edit.

**Defer the choice until the D7 producer lands.** Rejected: §D7 requires the
checkpoint *before* the dependency, and deferring would mean designing the
save/stale contract without knowing what the editor can express about a
conflict.

## Accepted costs

- **Weaker screen-reader support than Monaco.** This is real and is not
  mitigated by choosing CodeMirror. The D7 surface sub-phase owes an explicit
  keyboard-and-screen-reader pass over the editor pane, its tab strip, and the
  conflict dialog, and its report must state what was actually exercised rather
  than that ARIA attributes exist. §D7's acceptance already requires
  keyboard-only edit, find, save, close, and conflict resolution to pass tests;
  that bullet is now also this ADR's mitigation.
- **19 official languages, not 83.** Files outside that set fall back to
  `@codemirror/legacy-modes` or to plain text. A plain-text fallback is an
  honest outcome; what the surface may not do is imply a file was highlighted
  correctly when no grammar matched it.
- **A dependency surface of small packages rather than one.** `codemirror` 6.0.2
  pulls `@codemirror/{state,view,commands,search,autocomplete,language,lint}`
  and the `@lezer` parsers beneath them. All are MIT, all are from the same
  maintainer, and each is recorded individually in `THIRD_PARTY.md` when the
  surface sub-phase adds them — a single line naming only `codemirror` would
  hide the provenance AGENTS.md §8 exists to keep reviewable.
- **No language-service diagnostics in the editor.** Choosing CodeMirror makes
  adding one later a deliberate act rather than a flag flip, which is the
  intended trade and also a cost.
