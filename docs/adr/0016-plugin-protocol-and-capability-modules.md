# ADR-0016: Capability modules are first-party Rust; third-party code arrives via a governed plugin protocol

**Status:** Accepted
**Date:** 2026-08-18
**Deciders:** Jerrik Christiansen
**Related:** ADR-0002 (scripted extensions), ADR-0003 (shared core, clients), ADR-0006 (bounded workspace, trust classes), ADR-0014 (projections carry no authority), AGENTS.md §3 (security), §8 (provenance), Standing Law #2 (recall is a projection)

## Context

The master implementation plan for the next phase of mjolnr proposed a
"microkernel + plugin ecosystem": a `mjolnr-plugin.yaml` manifest format, a plugin
host with a hook lifecycle, `pass_env` for environment secrets, community
plugins shipping SvelteKit components mounted directly into the desktop
webview, and first-party capabilities (`@mjolnr/memory`, `@mjolnr/fleet`,
`@mjolnr/preview`) packaged as plugins.

Three things constrain that proposal:

1. **ADR-0002 already decided the extension surface.** Extensions are
   declarative exact-argv YAML files run through the ordinary `Execute`-gated
   command path; WASM and native-rebuild plugin surfaces were rejected. 0002
   left one explicit revisit clause: *"revisit only if extensions must compute
   rather than orchestrate."* Session lifecycle hooks and prompt-time
   annotation are compute, not orchestration — that clause is now triggered by
   the memory engine's consolidation needs and by plugins that want to observe
   the session. This ADR is that revisit, taken deliberately.
2. **The UX direction forbids unrestricted plugins.**
   `docs/ux-workspace-direction.md` ("Reject from Herdr") requires extensions
   to stay declarative, capability-scoped, session-loaded, and governed by the
   ordinary execution gate — no plugin may silently run with the user's full
   authority.
3. **The security rules are absolute.** Secrets never leave their boundary
   (AGENTS.md §3), which a blanket `pass_env` violates by construction, and
   third-party JavaScript with Tauri IPC access would be a second, ungoverned
   execution path inside a client that ADR-0003 confines to rendering
   snapshots and emitting commands.

mjolnr also has three working precedents that already solve most of this:
MCP (`src/mcp.rs` — `env_clear()` with an explicit pass-env allowlist, tools
namespaced `mcp:<server>:<tool>`, pinned at `ToolTier::Execute`, honest
`Unavailable` states), declarative extensions (`src/core/extension.rs`,
`src/context/extensions.rs`, evidenced load acts in
`src/runtime/durability.rs` — authorise-then-enable, tier not configurable
downward), and the integrations port (`src/integrations/` — secrets held in
`Secret` types, remote text framed as data, never authority).

The question this ADR settles: what is a "plugin" in mjolnr, what may third-party
code do, and how do first-party capabilities ship?

## Decision

**First-party capabilities are ordinary Rust modules in the single crate.
Third-party code is a plugin: a local subprocess speaking a versioned JSON-RPC
protocol over stdio, declared by a fail-closed manifest, with owner-granted
credentials, observer-only hooks, Execute-pinned tools, and data-only UI
contributions.**

### 1. Terminology: modules, plugins, MCP servers

| Term | What it is | Governing rule |
|---|---|---|
| **Capability module** | First-party Rust in the single crate (`src/memory/`, fleet logic in `src/runtime/`, preview behind typed desktop commands). Toggleable via declarable `.mjolnr/` files. | §2.2 extraction test; ordinary module boundaries; never called "plugin" in UI or docs |
| **Plugin** | Third-party code, local subprocess, versioned JSON-RPC over stdio (streamable-HTTP transport later, mirroring MCP's split). | This ADR |
| **MCP server** | External tool server, unchanged (`src/mcp.rs`). Tool-only: no lifecycle, no session context, no UI. | Existing implementation |

The overlap between plugins and MCP is intentional and not a redundancy: a
plugin may wrap an MCP server and add lifecycle, curation, or UI on top. MCP is
the floor — any tool provider can be an MCP server — and the plugin protocol is
the richer surface for publishers who want hooks and views. `@mjolnr/*` naming is
reserved for first-party modules; plugins use publisher namespacing
(`acme.deploy`).

### 2. The trust model is honest, not sandboxed

A plugin subprocess runs with the user's OS authority. mjolnr does **not** claim
OS-level sandboxing (AGENTS.md §3), and the plugin hub UI must say so plainly.
Trust is established by:

- **An owner install act**, recorded as a durable `SmedEvent`
  (`PluginInstalled`, later `PluginUpdated`) before any capability activates —
  the same authorise-then-enable ordering as extensions.
- **Pinned provenance**: source URL, publisher, and content hash recorded at
  install; an update changes the hash and requires re-approval.
- **A distinct trust class in the UI** (ADR-0006): plugin-owned surfaces are
  visually separate from mjolnr-governed and operator-controlled ones.

What mjolnr *mediates* is the plugin's access to mjolnr-owned capabilities: every
plugin tool call routes the ordinary policy gate, and hooks receive data, never
authority. Quality curation is a property of the ecosystem, not a security
claim — a curated-but-compromised plugin still has the user's authority, and
the ADR and the UI both say so.

### 3. The manifest is fail-closed and grants no tier

`mjolnr-plugin.yaml` declares identity, provenance, protocol version, tools
(name, description, JSON schema — no `$ref`, matching the registry's existing
validation), subscribed hooks, view descriptors, and credential grants.
Unknown fields refuse (the `deny_unknown_fields` pattern already used by
`McpConfig`). A plugin **cannot self-declare a safety tier**: every plugin tool
is pinned at `ToolTier::Execute`, exactly like MCP tools and extension tools,
so every call is gated, previewed, and evidenced.

### 4. Credentials are per-plugin owner grants — no `pass_env`

The manifest *names* the environment variables a plugin wants; it never
receives them by default. Each grant is approved by the owner at install or
first use, stored in mjolnr's owner-only credential files (one file per plugin,
the `src/store/secrets.rs` pattern), and injected as environment variables at
spawn only — never argv, never YAML, never logs, `Debug`-redacted, zeroized on
drop. Grant scope is exact variable names, displayed in the UI before
approval. This is the MCP `env_clear()` + allowlist discipline, extended with
per-grant owner consent.

### 5. Hooks are observers, full stop

`SessionStart`, `UserPromptSubmit`, `PreToolCall`, `PostToolCall`, `PostTurn`
receive structured JSON context and return **annotations only** — context
suggestions and notices. A hook cannot mutate tool arguments, widen policy,
approve anything, or veto a call. Tool-argument validation stays where it is,
after the last transformation in `src/runtime/tool_loop.rs` (`prepare_tool`),
unchanged, because hooks are never a transformation. A hook that wants to
change behaviour can only propose context; the model may weigh it and the gate
still disposes. If hook mutation is ever genuinely needed, that is a new ADR
arguing against this one.

### 6. UI contributions are data, not code

Plugins declare view descriptors in the manifest (e.g. `view: deployment-list`)
and emit JSON payloads. The desktop renders them with first-party generic
components (tables, status cards, timelines); the TUI renders the same
payloads as text, delivered through the client-bridge DTOs like every other
projection. **No third-party JavaScript ever loads in the Tauri webview.** This
keeps ADR-0003 intact (clients render snapshots, own no authority), keeps
ADR-0006's trust classes visible, and keeps the payload schema a reviewable
contract instead of an arbitrary code surface.

## What this does not license

- No plugin code in-process, ever. The protocol boundary is the containment
  boundary mjolnr actually has.
- No blanket environment inheritance of any kind — provider keys must not be
  inheritable by plugin subprocesses.
- No hook-authored tool arguments, approvals, or policy changes.
- No third-party components in the webview, however convenient the mount would
  be.
- No claim of sandboxing in UI, README, or marketing prose. The policy gate is
  a policy gate; the plugin trust model is curation plus explicit grants.
- Nothing here widens Standing Law #2: a plugin that builds recall aids
  (indexes, embeddings) owns a disposable projection, never durable truth.

## Rejected alternatives

- **The original microkernel draft** (community SvelteKit mounts, `pass_env`,
  self-declared tiers). Third-party JS with IPC access is a second, ungoverned
  execution path inside a client; `pass_env` hands secrets across their
  boundary by construction; self-declared tiers let a plugin opt itself out of
  the gate. All three violate prime directives, not just style.
- **A WASM sandbox host (wasmtime).** Real containment, but a heavy dependency
  and a from-scratch capability/host API for a need (compute in extensions)
  that hooks-as-observers largely covers. Rejected *for now*; see Revisit if.
- **MCP-only, no plugin protocol.** MCP carries no lifecycle, no session
  context, and no UI data; forcing plugins into it would produce protocol
  abuse (tools faking hooks) rather than less surface.
- **In-process first-party "plugins" loaded dynamically.** Rejected: it
  dissolves the single-crate extraction discipline (§2.2) and gives first-party
  code a second, weaker boundary for no benefit.

## Accepted costs

- The honest posture stated above: a compromised curated plugin has user OS
  authority. Mitigated by provenance pinning, explicit credential grants, and
  trust-class visibility; not eliminated.
- Subprocess lifecycle management (spawn, restart, cancellation via
  `CancellationToken`, bounded channels) is new runtime surface.
- The JSON-RPC protocol and its payload schemas are public contracts to
  maintain and version.
- Data-only UI means first-party generic components must cover plugin use
  cases; a genuinely novel visualization is a reason to extend the first-party
  component kit, not to admit plugin code.

## Enforcement

- A `Rule` entry for `src/plugins/` is added to `tests/architecture.rs::RULES`
  **before** the directory lands (precedent: rules armed ahead of
  `repository`/`integrations`). Forbidden for `plugins`: `tui`, `store`,
  `providers`.
- Negative tests are mandatory in the plugin phase: unknown manifest field
  refuses; undeclared tool call refuses; credential access without a grant
  refuses; a hook response attempting argument mutation is ignored and
  reported; a plugin subprocess crashing degrades to an honest unavailable
  state, never a silent pass-through.

## Revisit if

- A compute-sandbox need emerges — then re-evaluate WASM as a *second* runtime
  for untrusted-but-useful plugins, separate from the curated protocol here.
- Remote plugins are requested — streamable-HTTP transport under the same
  manifest, with stricter default grants and no local-spawn assumptions.
