# Third-party dependencies

Direct dependency inventory required by `AGENTS.md` §8. Mechanically enforced by `deny.toml` via `cargo deny check`.

mjolnr itself is licensed under [Apache-2.0](./LICENSE). Everything below is linked from crates.io under its own licence; no third-party source code is vendored into this repository. Because a copyleft dependency would force mjolnr to relicence rather than merely widen an allow list, `deny.toml` permits permissive licences only.

**Licence data below is read from `cargo metadata`, not from memory.** Regenerate rather than hand-edit when dependencies change:

```bash
cargo metadata --format-version 1 | jq -r '
  .packages[] | select(.name=="mjolnr") | .dependencies[].name' | sort -u
```

Last verified: 2026-08-18 — re-verified with `cargo deny check` against the four targets in `deny.toml`. No new runtime crates in 6.1/6.2 (Vercel/Supabase reuse `reqwest` + `serde_json`); last added crates remain `ratatui-image` and `image` (2026-07-25).

## Runtime dependencies

| Crate | Version | Licence | Purpose | Source |
|---|---|---|---|---|
| `ratatui` | 0.30.2 | MIT | TUI rendering. `TestBackend` backs the frame tests. | https://github.com/ratatui/ratatui |
| `crossterm` | 0.29.0 | MIT | Terminal backend, raw mode, alternate screen, input events. | https://github.com/crossterm-rs/crossterm |
| `tokio` | 1.52.3 | MIT | Async runtime, bounded channels, process spawning, timers. | https://github.com/tokio-rs/tokio |
| `tokio-util` | 0.7.18 | MIT | `CancellationToken` — the cancellation contract in `AGENTS.md` §4. | https://github.com/tokio-rs/tokio |
| `futures-util` | 0.3.32 | MIT OR Apache-2.0 | Stream combinators for provider response bodies. | https://github.com/rust-lang/futures-rs |
| `reqwest` | 0.13.4 | MIT OR Apache-2.0 | Provider HTTP with streaming bodies and OAuth exchanges. `default-features = false` + `rustls` — no OpenSSL. | https://github.com/seanmonstar/reqwest |
| `rmcp` | 2.2.0 | Apache-2.0 | Official MCP Rust SDK; governed stdio client transport only. | https://github.com/modelcontextprotocol/rust-sdk |
| `eventsource-stream` | 0.2.3 | MIT OR Apache-2.0 | Spec-compliant SSE decoding. See note below. | https://github.com/jpopesculian/eventsource-stream |
| `serde` | 1.0.228 | MIT OR Apache-2.0 | Provider wire types. | https://github.com/serde-rs/serde |
| `serde_json` | 1.0.150 | MIT OR Apache-2.0 | Provider bodies, tool arguments, JSON Schema values. | https://github.com/serde-rs/json |
| `serde_yaml_ng` | 0.10.0 | MIT OR Apache-2.0 | Strict typed parsing of standard Agent Skills YAML frontmatter. | https://github.com/acatton/serde-yaml-ng |
| `keyring` | 4.1.5 | MIT OR Apache-2.0 | Retained only so the one-shot migration can drain credentials written by earlier versions; storage is now an owner-only file. Comes out with `migrate_from_keyring`. | https://github.com/hwchen/keyring-rs |
| `jsonschema` | 0.47.0 | MIT | Draft 2020-12 validation of final tool arguments. External reference resolvers are disabled. | https://github.com/Stranger6667/jsonschema |
| `zeroize` | 1.9.0 | Apache-2.0 OR MIT | Wipes credentials from memory on drop. | https://github.com/RustCrypto/utils |
| `rpassword` | 7.5.4 | Apache-2.0 | Reads a credential from a terminal without echo. **Direct dependency justified below.** | https://github.com/conradkleinespel/rpassword |
| `clap` | 4.6.1 | MIT OR Apache-2.0 | CLI parsing. Secrets are never arguments. | https://github.com/clap-rs/clap |
| `sha2` | 0.11.0 | MIT OR Apache-2.0 | SHA-256 read-set versions for stale-edit refusal and completion evidence. | https://github.com/RustCrypto/hashes |
| `similar` | 3.1.1 | Apache-2.0 | Bounded unified diffs shown before write approvals. | https://github.com/mitsuhiko/similar |
| `syntect` | 5.3.0 | MIT | Syntax highlighting for transcript code blocks and diffs (Phase 20). Minimal features: `parsing`, `default-syntaxes`, `regex-fancy` — no C regex engine, no theme/asset loaders. See justification below. | https://github.com/trishume/syntect |
| `ratatui-image` | 11.0.6 | MIT | Image widget for the TUI: kitty / iTerm2 / sixel graphics protocols with a unicode-halfblock fallback. `default-features = false` + `crossterm` — no `chafa` C library, no extra image codecs. See justification below. | https://github.com/ratatui/ratatui-image |
| `image` | 0.25.10 | MIT OR Apache-2.0 | Decoding only, for the transcript's inline images. `ratatui-image` links it for PNG but does not re-export it. Codecs named individually — `png`, `jpeg`, `gif`, `webp` — rather than `default`. | https://github.com/image-rs/image |
| `tree-sitter-dart` | 0.2.0 | MIT | Dart grammar for AST-backed import and symbol extraction in the deterministic code graph. | https://github.com/nielsenko/tree-sitter-dart |
| `uuid` | 1.23.5 | MIT OR Apache-2.0 | Time-sortable v7 identifiers for messages, sessions, runs. | https://github.com/uuid-rs/uuid |
| `time` | 0.3.53 | MIT OR Apache-2.0 | Timestamps on canonical messages and stored events. | https://github.com/time-rs/time |
| `thiserror` | 2.0.18 | MIT OR Apache-2.0 | Typed errors per module. No `anyhow` in library code. | https://github.com/dtolnay/thiserror |
| `async-trait` | 0.1.89 | MIT OR Apache-2.0 | Object-safe plugin boundaries only (`Provider`, `Tool`, `EventStore`). | https://github.com/dtolnay/async-trait |
| `tokio-rusqlite` | 0.7.0 | MIT | Durable events and checkpoints. Owns the SQLite connection thread. `bundled` compiles SQLite in — see below. | https://github.com/programatik29/tokio-rusqlite |
| `etcetera` | 0.11.0 | MIT OR Apache-2.0 | Resolves the platform data directory without introducing a disallowed licence. **See below.** | https://github.com/lunacookies/etcetera |
| `unicode-normalization` | 0.1.25 | MIT OR Apache-2.0 | NFKC normalization required by the official Agent Skills validation fixtures. | https://github.com/unicode-rs/unicode-normalization |
| `tauri` | 2.11.5 (declared 2.1.1, tauri-build 2.6.3) | MIT OR Apache-2.0 | Tauri 2 framework hosting mjolnr's shared core & client bridge. | https://github.com/tauri-apps/tauri |

### Phase A0 Tauri Desktop Client Dependencies (`desktop/package.json`)

| Package | Version | Licence | Purpose | Source |
|---|---|---|---|---|
| `svelte` | 5.2.0 | MIT | Svelte 5 frontend framework for Tauri SPA client | https://github.com/sveltejs/svelte |
| `@sveltejs/kit` | 2.16.0 | MIT | SvelteKit static SPA routing | https://github.com/sveltejs/kit |
| `@sveltejs/adapter-static` | 3.0.8 | MIT | Prerenders static SPA bundle for Tauri `frontendDist` | https://github.com/sveltejs/kit |
| `@sveltejs/vite-plugin-svelte` | 5.0.0 | MIT | Svelte integration plugin for Vite | https://github.com/sveltejs/vite-plugin-svelte |
| `@tauri-apps/api` | 2.1.1 | MIT OR Apache-2.0 | Tauri 2 IPC invoker and event listener bindings | https://github.com/tauri-apps/tauri |
| `vite` | 6.4.3 | MIT | Frontend build tool | https://github.com/vitejs/vite |
| `vitest` | 3.0.0 | MIT | Frontend unit and component testing runner | https://github.com/vitest-dev/vitest |
| `svelte-check` | 4.1.0 | MIT | Diagnostic checker for Svelte/TypeScript codebases | https://github.com/sveltejs/language-tools |
| `typescript` | 5.7.3 | Apache-2.0 | Type checking and compiler diagnostics | https://github.com/microsoft/TypeScript |
| `eslint` | 9.18.0 | MIT | Linter engine for JavaScript and TypeScript | https://github.com/eslint/eslint |
| `typescript-eslint` | 8.65.0 | MIT | TypeScript ESLint tooling wrapper | https://github.com/typescript-eslint/typescript-eslint |
| `@typescript-eslint/eslint-plugin` | 8.20.0 | MIT | TypeScript lint rules for ESLint 9 | https://github.com/typescript-eslint/typescript-eslint |
| `@typescript-eslint/parser` | 8.20.0 | MIT | TypeScript AST parser for ESLint 9 | https://github.com/typescript-eslint/typescript-eslint |
| `eslint-plugin-svelte` | 2.46.0 | MIT | ESLint plugin for Svelte 5 component syntax | https://github.com/sveltejs/eslint-plugin-svelte |
| `globals` | 17.8.0 | MIT | Global identifiers for ESLint environment config | https://github.com/sindresorhus/globals |
| `jsdom` | 26.0.0 | MIT | DOM environment simulation for Vitest component tests | https://github.com/jsdom/jsdom |
| `@testing-library/dom` | 10.4.0 | MIT | DOM testing utilities for component tests | https://github.com/testing-library/dom-testing-library |
| `@testing-library/svelte` | 5.2.4 | MIT | Svelte component testing utilities | https://github.com/testing-library/svelte-testing-library |
| `@types/node` | 22.10.0 | MIT | Node.js type definitions | https://github.com/DefinitelyTyped/DefinitelyTyped |
| `shadcn-svelte` | 1.4.2 | MIT | CLI and registry source for the standard desktop component system | https://github.com/huntabyte/shadcn-svelte |
| `bits-ui` | 2.18.1 | MIT | Accessible interaction primitives beneath generated shadcn-svelte components | https://github.com/huntabyte/bits-ui |
| `tailwindcss` | 4.3.3 | MIT | Utility compiler and semantic theme layer used by generated components | https://github.com/tailwindlabs/tailwindcss |
| `@tailwindcss/vite` | 4.3.3 | MIT | Tailwind 4 integration for the existing Vite build | https://github.com/tailwindlabs/tailwindcss |
| `tailwind-variants` | 3.3.0 | MIT | Typed variants used by generated component source | https://github.com/heroui-inc/tailwind-variants |
| `tailwind-merge` | 3.6.0 | MIT | Deterministic merging of generated component utility classes | https://github.com/dcastil/tailwind-merge |
| `clsx` | 2.1.1 | MIT | Conditional class composition used by the shadcn `cn` helper | https://github.com/lukeed/clsx |
| `tw-animate-css` | 1.4.0 | MIT | Tailwind 4 animation utilities for overlays and disclosure components | https://github.com/Wombosvideo/tw-animate-css |
| `@hugeicons/svelte` | 1.1.4 | MIT | Svelte renderer for the configured HugeIcons component set | https://github.com/hugeicons/hugeicons |
| `@hugeicons/core-free-icons` | 4.2.3 | MIT | Free HugeIcons glyph data selected for the desktop system | https://github.com/hugeicons/hugeicons |
| `@fontsource-variable/inter` | 5.3.0 | OFL-1.1 | Self-hosted Inter variable font for prose, per ADR-0007's Obsidian Cyan token palette | https://github.com/fontsource/font-files |
| `@fontsource-variable/jetbrains-mono` | 5.3.0 | OFL-1.1 | Self-hosted JetBrains Mono variable font for code/revisions/hashes/paths, replacing Geist Mono per ADR-0007 | https://github.com/fontsource/font-files |
| `mode-watcher` | 1.1.0 | MIT | Class-based light/dark mode synchronization used by generated components | https://github.com/svecosystem/mode-watcher |
| `svelte-sonner` | 1.1.1 | MIT | Accessible toast implementation exposed through the generated Sonner component | https://github.com/wobsoriano/svelte-sonner |
| `paneforge` | 1.0.2 | MIT | Keyboard-accessible resizable layout primitive available to workspace surfaces | https://github.com/svecosystem/paneforge |
| `@xyflow/svelte` | 1.6.3 | MIT | Interactive graph canvas for bounded node rendering, pan/zoom, dragging, selection, controls, and minimap in the Svelte 5 desktop client. | https://github.com/xyflow/xyflow |
| `d3-force` | 3.0.0 | ISC | Deterministic force-directed layout and collision resolution for the Yggdrasil code graph. | https://github.com/d3/d3-force |
| `@types/d3-force` | 3.0.10 | MIT | TypeScript declarations for the D3 force layout used by the desktop graph surface. | https://github.com/DefinitelyTyped/DefinitelyTyped |
| `@internationalized/date` | 3.12.2 | Apache-2.0 | Date value types required by the installed Bits UI component graph | https://github.com/adobe/react-spectrum |
| `codemirror` | 6.0.2 | MIT | CodeMirror 6 editor facade for the governed workspace file surface. | https://github.com/codemirror/dev |
| `@codemirror/state` | 6.7.1 | MIT | Immutable editor state and document transactions. | https://github.com/codemirror/dev |
| `@codemirror/view` | 6.43.7 | MIT | Accessible contenteditable editor view and rendering. | https://github.com/codemirror/dev |
| `@codemirror/commands` | 6.10.4 | MIT | History, indentation, and standard editor commands. | https://github.com/codemirror/dev |
| `@codemirror/language` | 6.12.4 | MIT | Language-support extension primitives. | https://github.com/codemirror/dev |
| `@codemirror/search` | 6.7.1 | MIT | Bounded in-editor find commands. | https://github.com/codemirror/dev |
| `@codemirror/autocomplete` | 6.20.3 | MIT | CodeMirror completion support pulled by the accepted editor surface. | https://github.com/codemirror/dev |
| `@codemirror/lint` | 6.9.7 | MIT | Editor diagnostic rendering boundary; diagnostics remain Rust-owned. | https://github.com/codemirror/dev |
| `@codemirror/lang-rust` | 6.0.2 | MIT | Rust syntax highlighting for workspace files. | https://github.com/codemirror/dev |
| `@codemirror/lang-javascript` | 6.2.5 | MIT | JavaScript and TypeScript syntax highlighting for workspace files. | https://github.com/codemirror/dev |
| `@codemirror/lang-json` | 6.0.2 | MIT | JSON syntax highlighting for workspace files. | https://github.com/codemirror/dev |
| `@codemirror/legacy-modes` | 6.5.3 | MIT | Explicit plain/legacy fallback boundary for languages without a selected Lezer grammar. | https://github.com/codemirror/dev |


### Phase C standard desktop component system

**Purpose:** shadcn-svelte supplies a coherent, accessible Svelte component
baseline so mjolnr's desktop effort stays focused on governed runtime workflows
rather than maintaining buttons, dialogs, selectors, sidebars, tabs, command
menus, and focus behavior. The committed source uses the Nova style with a zinc
base/theme, HugeIcons, and the default translucent menu treatment. Typography
and radius follow ADR-0007's Obsidian Cyan token palette (Inter/JetBrains
Mono, the mockup's radius scale) rather than the original Geist Mono/small
radius default — a values-only change to `desktop/src/app.css`, not a
component-system change.

**Alternatives rejected:** continuing the Phase C hand-built primitive set was
rejected because it duplicated mature accessibility and interaction work and
would leave mjolnr owning a bespoke UI library. A hybrid custom/Bits UI layer
was also rejected because it would preserve two component conventions. The
generated shadcn-svelte source is the sole desktop component baseline.

**Licence:** package metadata reports MIT for the component and utility
packages, Apache-2.0 for `@internationalized/date`, and OFL-1.1 for the
Inter/JetBrains Mono variable fonts. No provider, runtime, policy, or
persistence boundary depends on these packages.

**Removal cost:** moderate and confined to `desktop/`. Replacing the system
would require rewriting presentation composition, but not Rust runtime truth,
client DTOs, plan authority, approval, recovery, or execution policy.

### Yggdrasil graph surface

**Purpose:** `@xyflow/svelte` owns the accessible 2D viewport interactions and
custom node surface; `d3-force` computes the bounded, clustered layout from the
Rust-owned graph projection. The frontend receives no authority from either
package and does not infer temporal events.

**Alternatives rejected:** Three.js was rejected because the target reference
is a 2D knowledge map and a 3D scene would add depth-navigation and rendering
complexity without improving the required pan/zoom/drag workflow. A hand-built
SVG viewport was rejected because it already failed to provide usable zoom,
pan, minimap, node dragging, and visual hierarchy.

**Licence:** `@xyflow/svelte` is MIT and `d3-force` is ISC according to the
installed package metadata; both are permissive and confined to `desktop/`.

**Removal cost:** moderate and isolated to the graph surface and its package
lockfile. The Rust graph contract remains renderer-independent.


### Phase 11 official MCP client

**Purpose:** `rmcp` supplies the official MCP handshake, paginated tool discovery,
tool-call wire types, cancellation, and bounded child-process shutdown. mjolnr
enables only its client and child-process features; HTTP and OAuth transports
are outside Phase 11.

**Alternatives rejected:** a hand-written JSON-RPC/MCP implementation would make
mjolnr own protocol version negotiation, cancellation, pagination, and framing.
Using an unofficial SDK was rejected by `AGENTS.md` section 8.

**Licence:** Apache-2.0, read from the downloaded 2.2.0 crate metadata; it passes
`cargo deny`.

**Removal cost:** moderate and isolated to `src/mcp.rs`, the composition seam,
and the MCP status DTO. Replacing it must preserve process cleanup and the
scripted stdio contract tests.

### Phase 5 frontmatter and Unicode conformance

**Purpose:** `serde_yaml_ng` decodes the published `SKILL.md` frontmatter fields with unknown-field and type rejection; `unicode-normalization` makes composed and decomposed Unicode skill names compare consistently, matching the official reference fixtures.

**Alternatives rejected:** a hand-written YAML subset was removed because it could reject valid YAML scalars or silently diverge as the standard evolves. Hand-written Unicode normalization was rejected because Unicode Standard Annex #15 requires generated character tables and a conformance algorithm, not a safe local approximation.

**Licence:** both are MIT OR Apache-2.0 and pass `cargo deny`.

**Removal cost:** low and confined to `src/context/frontmatter.rs`; replacement must preserve typed YAML validation and NFKC fixture parity.

### `rpassword`: no-echo credential input

**Purpose:** `mjolnr auth login` reads an API key from the terminal. Without no-echo input the key is echoed into terminal scrollback — and into whatever the user's terminal or session recorder logs. `AGENTS.md` §3 says a secret must never reach the screen, so plain `stdin` reading is not an option for an interactive prompt.

**Alternatives rejected:**

- *Read from plain stdin.* Echoes the key. Rejected on §3.
- *Environment variable only.* Forces the key into the shell's environment and history — worse than the problem being solved.
- *Hand-roll no-echo via `crossterm`* (already a dependency, and it can disable echo). Rejected: this is termios handling that must restore echo across panics and signals. It is precisely the code that looks trivial and then leaves a user's terminal unable to echo. Not worth owning to avoid a 200-line Apache-2.0 crate.

**Licence:** Apache-2.0 (with `rtoolbox`, Apache-2.0). Both pass `cargo deny`.

**Removal cost:** low — one call site in `src/cli/auth.rs::login`.

### Why `eventsource-stream` rather than hand-rolling

Not convenience. `docs/provider-contract.md` §4 records that OpenRouter emits SSE comment frames (`: OPENROUTER PROCESSING`) as keep-alives. A naive `split("data: ")` parser crashes on them; a spec-compliant decoder skips them by construction. The transport decoder also stays separate from the provider event decoder, which the provider contract requires.

### Why `rustls` rather than the default TLS

`default-features = false` avoids `native-tls`/OpenSSL, keeping the static-binary distribution promise from `docs/adr/0001-all-rust-ratatui.md` — no system OpenSSL to link, version-match, or CVE-chase at install time.

**Feature-name drift caught in Phase 0:** reqwest 0.13 renamed the `rustls-tls` feature to `rustls`. The 0.12 name fails to resolve. Recorded because it is exactly the kind of drift that makes a copied snippet fail confusingly.

### `etcetera` instead of `directories`

`directories 6` cannot be used under the repository's permissive-only licence
policy.

`directories 6` → `dirs-sys 0.5` → `option-ext 0.2`, which is **MPL-2.0**. `cargo deny check` refuses it:

```text
error[rejected]: failed to satisfy license requirements
   ┌─ option-ext-0.2.0/Cargo.toml:21:12
21 │ license = "MPL-2.0"
   │            ━━━━━━━  rejected: license is not explicitly allowed
```

The allowlist is intentionally not widened for a convenience dependency. mjolnr
is Apache-2.0 and keeps its dependency graph permissive so downstream use does
not acquire an unexpected relicensing obligation.

**Purpose:** resolve the platform-appropriate user data directory for the
durable store. mjolnr uses `choose_native_strategy`, not `choose_app_strategy`:
the latter uses XDG on macOS too, which is a defensible CLI convention but not
the platform-native location.

| Platform | Data directory |
|---|---|
| macOS | `~/Library/Application Support/mjolnr` |
| Linux | `$XDG_DATA_HOME/mjolnr`, else `~/.local/share/mjolnr` |

**Alternatives rejected:**

- *`directories 6`* (the shortlisted crate). Rejected on the MPL-2.0 transitive dependency above.
- *Widen the licence allowlist.* Rejected because dependency convenience does
  not justify changing the repository's licence policy.
- *`dirs 6`.* Same `option-ext` dependency, same refusal.
- *Hand-roll it.* Tempting at roughly 25 lines for two platforms, and genuinely testable as a pure function. Rejected because the rules are small but not guessable — XDG ignores a relative `XDG_DATA_HOME`, macOS wants `Application Support` — and the failure mode is a dotfile quietly written to the wrong place on a platform the author was not using. Not worth owning to avoid two permissive crates.

**Licence:** MIT OR Apache-2.0, with `cfg-if` (MIT/Apache-2.0) and `windows-sys` (MIT/Apache-2.0). No copyleft anywhere in its graph. `cargo deny check` passes.

**Removal cost:** low — one call site in `src/store/paths.rs::default_database_path`, behind a function every other caller goes through.

### `tokio-rusqlite`, and why `bundled`

 shortlists it and warns: "use its compatible re-export; do not add mismatched `rusqlite`." That warning is load-bearing. `tokio-rusqlite 0.7` pins `rusqlite ^0.37`, while the current release is 0.40 — adding `rusqlite = "0.40"` alongside it would resolve a **second `libsqlite3-sys`**, i.e. two SQLite libraries linked into one binary. `cargo tree -i rusqlite` must show exactly one version, reached only through `tokio-rusqlite`.

**Purpose:** durable events and checkpoints, on a dedicated connection thread so SQLite never blocks a Tokio worker (`AGENTS.md` §4).

**`bundled`** compiles SQLite from source rather than linking the host's `libsqlite3`. The WAL, `busy_timeout`, and `integrity_check` behaviour mjolnr tests against is then the behaviour that ships — the same reasoning as `rustls` over system OpenSSL (`docs/adr/0001-all-rust-ratatui.md`). Cost: a C compiler at build time and a slower cold build.

**Its queue is unbounded** (`crossbeam_channel::unbounded`, `src/lib.rs:376`), so it is never mjolnr's ordering or backpressure boundary. `src/store/sqlite/actor.rs` puts a bounded actor in front; see `docs/persistence.md` §1.3.

**Alternatives rejected:**

- *`sqlx`.* A query framework with compile-time checking and a migration runner. Rejected: it brings a connection pool and a macro layer for a single-writer, single-connection local file, and the persistence design wants one bounded writer, not a pool.
- *Raw `rusqlite` + `spawn_blocking` per call.* Rejected: that is `tokio-rusqlite`'s job, and hand-rolling it re-creates the thread lifecycle without the review the crate has had.

**Removal cost:** moderate — confined to `src/store/sqlite/`, behind the `EventStore` port the runtime actually depends on.

### Phase 3 validation, hashing, and diff dependencies

- **`jsonschema`:** validates each proposal and validates again immediately before execution. Hand-written per-tool validators were rejected because two validation implementations would drift from the schemas sent to providers. Removal cost is moderate: replace the registry validator while preserving Draft 2020-12 behavior and every negative test.
- **`sha2`:** supplies stable SHA-256 file versions. `DefaultHasher` was rejected because it is neither stable across implementations nor intended as durable evidence. Removal cost is low behind `tools::files::hash`.
- **`similar`:** creates reviewable unified diffs without interpreting them as patches. A hand-rolled line diff was rejected because approval quality depends on correct additions, removals, and context. Removal cost is low behind `tools::files::review_diff`.

All three were shortlisted during the dependency review, are permissively licensed, and pass `cargo deny`.

### Phase 20 highlighting dependency

- **`syntect`:** highlights fenced code blocks and unified diffs in the transcript. Purpose: the transcript is where a coding harness is judged; flat-green code was the largest perceived-quality gap. **Alternatives rejected:** tree-sitter (per-language C grammars compiled in — far heavier than the value it adds); hand-rolled keyword coloring (mediocre output and a permanent maintenance sink); `pulldown-cmark` ( shortlisted it for parsing, but parsing was never the gap — token coloring is). **Licence:** MIT. **Feature cut:** `default-features = false` with `parsing`, `default-syntaxes`, `regex-fancy` — the pure-Rust regex engine, no oniguruma C library, no theme/plist/html loaders (mjolnr builds its own palette from `tui::theme`). **Advisory note:** pulls `bincode 1.3.3` (RUSTSEC-2025-0141, unmaintained-only, no safe upgrade) to deserialize its own crate-bundled binary syntax assets — trusted data, recorded in `deny.toml`. **Removal cost:** low — confined to `src/tui/highlight.rs`; deleting it returns the transcript to flat code blocks with no API change elsewhere.

### Terminal image rendering

- **`ratatui-image`:** renders images inside the Ratatui frame using the terminal's own graphics protocol (kitty, iTerm2, sixel), falling back to unicode half-blocks where none is available. **Licence:** MIT. **Feature cut:** `default-features = false` with only `crossterm`. The default set pulls `chafa-dyn`, which links the system `libchafa` through `pkg-config` — a C system dependency, the same thing the `syntect` `regex-fancy` choice above exists to avoid — and `image-defaults`, which widens `image` to every codec it ships. What remains decodes PNG, which is what the widget itself requires; wider codec support is the separate `image` entry below. **Version fit:** depends on `ratatui ^0.30.1`, so it shares mjolnr's single `ratatui 0.30.2` (`cargo tree -i ratatui` shows one). **Duplicate note:** brings `thiserror 1.0.69` alongside the workspace's `2.0.18` — a `cargo deny` *warning* under `multiple-versions = "warn"`, not a failure. 

**Call sites (2026-07-25):** `src/tui/image.rs` owns protocol detection, containment, decode, and encoding; `src/tui/timeline.rs` reserves the rows and draws. The live source of links is the pasted `![…](file://…)` that `src/tui/app.rs` writes into the composer — `ContentBlock::ImageRef` exists in `core` but is constructed nowhere, so it is not the path. **Removal cost:** low and localised — `src/tui/image.rs` plus the placeholder/draw hooks in `timeline.rs`; the transcript returns to captioned links.

- **`image`** arrives with it, as decoding only. `ratatui-image`'s own `image` feature covers PNG for its internals but the crate is not re-exported, so the decode call needs a direct handle — which is also the place to choose codecs deliberately. Every decoder is attack surface reached from a path named in a message, so `default` is off and four formats are named: `png` (pasted screenshots), `jpeg`, `gif`, `webp`. Decode is additionally bounded by an 8 MiB file cap and an 8192-pixel edge limit before any pixels are allocated.

### E3 syntax graph breadth

- **`tree-sitter`, `tree-sitter-javascript`, `tree-sitter-typescript`, `tree-sitter-python`, and `tree-sitter-go`:** parse bounded workspace source files into deterministic import and definition facts for the code graph. The grammar crates are used as syntax parsers only; they do not execute project code, resolve dependencies, or rank results. **Alternatives rejected:** extending the Rust line scanner to every language (it would silently accept malformed syntax and make each grammar a new hand-rolled parser); semantic language servers or compiler metadata (they would add process execution, tool-tier, and workspace-state concerns to a read-only graph); embeddings or learned retrieval (forbidden by `AGENTS.md` §11). **Licence:** MIT. **Removal cost:** moderate — remove the five direct dependencies and the foreign-language extraction/resolution module while retaining the existing Rust graph and query surface.

### D8 operator terminal

- **`portable-pty`:** owns the platform PTY allocation, bounded process launch, resize, input, and termination boundary for the operator terminal. **Alternatives rejected:** piping `std::process::Command` (not a terminal: it cannot model interactive programs, resize, or alternate screens); a frontend-owned shell (would bypass Rust lifecycle and environment controls); `alacritty_terminal` (larger terminal stack than this checkpoint needs). **Licence:** MIT. **Removal cost:** moderate — replace the PTY session adapter while preserving the core client DTOs and manager contract.
- **`vt100`:** parses bounded PTY bytes into a deterministic screen projection, including ANSI cursor movement and alternate-screen behavior, without exposing raw terminal control bytes to the frontend. **Alternatives rejected:** returning raw output (the frontend would become a second terminal emulator and unbounded scrollback sink); hand-rolled ANSI parsing (a correctness and security surface with no removal advantage). **Licence:** MIT. **Removal cost:** low to moderate — replace the screen projection while preserving process ownership and bounds.

## Development dependencies

Not linked into release artifacts.

| Crate | Version | Licence | Purpose | Source |
|---|---|---|---|---|
| `proptest` | 1.11.0 | MIT OR Apache-2.0 | Property tests for arbitrary parent-path escape attempts. | https://github.com/proptest-rs/proptest |
| `wiremock` | 0.6.5 | MIT/Apache-2.0 | Local mock HTTP for provider contract tests. Keeps the default test run offline (`AGENTS.md` §7). | https://github.com/LukeMathWalker/wiremock-rs |
| `serde_json` | 1.0.150 | MIT OR Apache-2.0 | Decoding provider bodies in the Phase 0 stream spike. Promotes to a runtime dependency in Phase 2. | https://github.com/serde-rs/json |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 | Isolated disposable repositories and databases for guarded-loop, path, process, and persistence tests. | https://github.com/Stebalien/tempfile |
| `tokio-rusqlite` | 0.7.0 | MIT | Also a dev-dependency — see below. | https://github.com/programatik29/tokio-rusqlite |

`proptest` replaces a finite hand-picked parent-path list with generated negative cases; removal cost is low and limited to tests. `tempfile` owns reliable cleanup and collision-free paths; hand-managed directories under `/tmp` were rejected because failed tests leave state that can couple later runs. Its removal cost is low and test-only.

**`tokio-rusqlite` is listed twice on purpose.** An integration test sees only the crate's public API plus dev-dependencies, and `tests/persistence_store.rs` must corrupt a database *out of band* — insert a duplicate event id, delete an event to open a sequence gap, bump `user_version` to a future schema — to prove those guards refuse. Those are the tests most worth having and they cannot be written through the store's own API, which exists to prevent exactly those states. The dev entry resolves to the same version already in the graph; `cargo tree -i rusqlite` still shows one.

### Deliberately not yet added

 shortlists these, but 's anti-patterns forbid adding a crate before its phase needs it. They arrive with the phase that uses them:

| Crate | Arrives in | For |
|---|---|---|
| `tracing`, `tracing-subscriber` | 8 | Redacted file logging with bounded rotation |
| `pulldown-cmark` | 8 | Markdown rendering |

`directories` was scheduled here for Phase 8 and is now **removed from the plan entirely**: Phase 4 needed platform paths, and `directories` cannot pass `cargo deny` (see above). `etcetera` does that job.

Removed on review for having no call sites: `proptest` and `tempfile` (Phase 0), `tracing` and `tracing-subscriber` (Phase 2 — they appeared only in a comment explaining *why* a `Debug` leak is dangerous, which is not a use). `tokio`'s `process` and `io-util` features were trimmed for the same reason; they serve Phase 3.

### Feature additions in Phase 4

- **`time`** gains `serde-well-known`: stored timestamps are RFC3339 text ('s `occurred_at TEXT`), and the alternative was hand-rolling a format the database schema already specifies.
- **`uuid`** gains `serde`: identifiers round-trip through the persisted envelopes.

## Licence posture

All direct dependencies are permissive (MIT / Apache-2.0 / dual). `deny.toml` allows only permissive licences across the whole linked graph, trimmed to those actually encountered.

**mjolnr is Apache-2.0** (owner decision, 2026-07-31). The permissive-only constraint predates that choice — it existed to keep every option open while the decision was pending — and it survives the choice for a stronger reason: a copyleft dependency would now force a relicence, not merely narrow a pending decision. A new licence appearing in the graph fails CI on purpose.

Both manifests declare `license = "Apache-2.0"`. `deny.toml` no longer sets `private = { ignore = true }`, so `mjolnr` is checked against the allowlist like any other package in the graph — `publish = false` does not buy an exemption from the policy this file exists to enforce. Publishing to crates.io remains a separate decision that has not been made.

## Not dependencies

Recording the negative space, since `AGENTS.md` §8 makes provenance reviewable from the first commit:

- **No unofficial provider SDKs.** Adapters are written against documented REST contracts (`docs/provider-contract.md`).
- **No git dependencies** — `deny.toml` sets `allow-git = []`. A git dependency is the easiest way to vendor agent-repository code by accident.
- **No code from the researched agent repositories** (Pi, Oh My Pi, OpenCode, OpenTUI, OpenGUI, Wayland, Codex, Claude Code, Hermes). They informed requirements and known failure modes only. Nothing is copied, ported, translated, or refactored from them.

## Adding a dependency

`AGENTS.md` §8: record purpose, licence, rejected alternatives, and removal cost in the report; add the row here; confirm `cargo deny check` passes. Prefer the standard library where it stays clear and safe.

This is the whole gate. 's shortlist table is a planning-time record, **not** an allowlist — it has been overridden five times, each correctly, and its status note says so. A dependency may land at most one step ahead of its call sites when the crate choice is the thing under review, provided its entry here says so plainly. An unused dependency with no recorded intent is still removed on review.

If third-party code is ever deliberately incorporated as source rather than as a dependency, preserve its notice, comply with its licence, and record it in an ADR — plainly, not obscured.
