# ADR 0002 — Scripted shim for agent-authored tool extensions

- **Status:** accepted
- **Date:** 2026-07-22
- **Decider:** Jerrik
- **Context:** `docs/tool-policy.md`; `docs/adr/0001-all-rust-ratatui.md`

## Decision

An agent-authored tool extension is a declarative file: a name, a description, a
set of string parameters, and one exact-argv command template with `${name}`
placeholders. Invoking the extension substitutes the validated arguments into the
argv and runs it through the same command execution path smed already owns
(`tools::command::run_process`), gated at `Execute` tier like any other command.

smed does **not** load agent-authored WASM, and does **not** treat a native Rust
addition (which needs a rebuild) as the extension surface. The extension is text
that names a command; the command is what runs, and it runs behind the gate that
already stands in front of every command.

## Context

 makes the implementation surface an explicit open question the
implementer must resolve and document rather than assume:

> the implementation surface, e.g. a scripted/WASM shim versus a native Rust
> addition requiring a rebuild, is an open design question the implementer must
> resolve and document, not assume

The phase's own requirements bound the answer. A written extension must be
**inert until an explicit load act** and then become **callable in the running
session** (§Phase 17, "Explicit load step"). A loaded extension tool must resolve
to an ordinary policy tier and be "previewed, gated, evidenced, and refusable
exactly like built-ins" ("Loaded tools are ordinary tools"), defaulting to
`Execute` per the MCP precedent from Phase 11. And the whole phase exists under
smed's identity: the model proposes, deterministic code disposes (`AGENTS.md`
§1), and every consequential act is gated and evidenced.

## Rationale

**The command path is already the gate the plan asks for.** `run_command` is an
`Execute`-tier tool: its calls are previewed (`CommandSpec::display`), gated by
policy, revalidated against workspace identity at the effect boundary, run with a
cleared and secret-scrubbed environment, bounded in output and time, and
cancellable down the process group. A scripted extension is a *named, argument-
shaped view* onto that same path. It inherits every one of those guarantees for
free, because it literally calls the same `run_process`. Nothing about "loaded
tools are ordinary tools" has to be rebuilt — it is true by construction.

**Exact argv, never a shell.** smed's tool boundary already refuses shell
strings: `CommandSpec` is a program plus an argument vector, and
`CommandSpec::display` quotes for humans without ever producing shell syntax that
expands twice (`src/core/tool.rs`). Extension substitution is whole-value: a
`${path}` placeholder is replaced inside a single argv element and never splits
into more. There is no interpreter to inject into, because there is no
interpreter — the model's authored argv reaches `execvp` as written.

**Live load without a second loader.** A scripted extension is data. Registering
it into the running `ToolRegistry` is an in-process act — it does not need a
rebuild, a subprocess supervisor, or a sandbox host. That is what makes the
plan's "explicit load step makes it callable in-session" achievable at all, and
it reuses 16.5's `/reload` discovery gate rather than standing up the second
resource loader this plan keeps refusing.

## Alternatives rejected

**WASM shim.** An extension compiled to WebAssembly, loaded into an embedded
runtime (wasmtime or similar). Genuinely sandboxable and live-loadable, and the
right answer if extensions ever needed to run untrusted *computation* rather than
orchestrate existing commands. Rejected for this phase: it adds a large runtime
dependency, a host ABI, and a capability-plumbing story — and it does **not**
reuse the command path, so "loaded tools are gated exactly like built-ins" would
have to be re-established for a second execution surface instead of inherited.
The cost buys isolation smed does not yet need, because the thing an extension
does — run a bounded command at the workspace root — is already isolated by the
`Execute` gate. Revisit if extensions must compute rather than orchestrate.

**Native Rust addition requiring a rebuild.** Maximally powerful: a new tool is a
new `impl Tool`. Rejected as the *extension* surface because it cannot satisfy
the phase's core requirement — a rebuild is not an in-session load, and gating
"the explicit act that made this callable" collapses when the act is `cargo
build`. Native tools remain how smed's *maintainers* add capability; they are
not how smed's *agent loop* proposes one.

## Consequences

Accepted, stated so they are not rediscovered as surprises:

- An extension can do exactly what `run_command` can do, and no more or less. It
  cannot read files into the model, hold state, or make network calls except by
  invoking a program that does — and every such program is itself subject to the
  same `Execute` gate. This is a ceiling, not a bug: a skill is knowledge, an
  extension is a *named command*, and neither acquires capability by being
  written convincingly (§Phase 17 anti-patterns).
- The extension's power is bounded by what is on `PATH` and executable at the
  workspace root. An extension naming a program that is not installed fails at
  spawn exactly as `run_command` would, with the same message.
- Substitution is string-only. Numeric or structured parameters are out of scope;
  the argv is strings, so the parameters are strings. A future typed-parameter
  need is a definition-format change, not an execution-surface change.
- Because the extension *is* a command view, its default tier is `Execute` and is
  not configurable downward by the definition. An extension cannot declare itself
  `Read` to dodge a gate; provenance is unknown, so the fail-closed tier stands
  (`ToolTier::default`, `docs/tool-policy.md`).

## Revisit if

- Extensions need to run untrusted computation rather than orchestrate existing
  commands — that is the WASM case, and it reopens honestly.
- A real need appears for extension tools with non-string parameters or with
  effects the command path cannot express (structured file edits, in-process
  state). That is a definition-format and possibly an execution-surface change,
  and this ADR should be superseded rather than stretched.
