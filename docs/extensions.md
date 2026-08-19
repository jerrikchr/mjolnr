# Agent-authored tool extensions

> **Sibling system:** Third-party plugins are JSON-RPC stdio subprocesses per [ADR-0016](adr/0016-plugin-protocol-and-capability-modules.md) — see [Master Plan §3](../docs/master-implementation-plan.md), `src/cli/plugin.rs` (`smed plugin create <name> [--template node|rust|python] [--yes]`, `smed plugin list`), and per-file manifests `.smed/plugins/*.yaml` discovered by `src/context/plugins.rs`. Flagship example `examples/plugins/vercel-deployments/`. Extensions below are declarative argv templates; plugins are `Execute`-pinned subprocess tools with observer-only hooks.

Phase 17 lets smed's agent loop propose a new tool and — only after an explicit
human act — make it callable. An extension is **data, not code**: a declarative
argv template smed runs through the command path it already gates. The design
and the alternatives considered are in
[`adr/0002-scripted-extension-shim.md`](adr/0002-scripted-extension-shim.md).

## The file

An extension is one YAML file at `.smed/extensions/<name>.yaml`. The file stem
is the tool name; the two may not disagree.

```yaml
name: count-lines
description: Count the lines in a file at the workspace root.
parameters:
  - name: path
    description: File to count, relative to the workspace root.
run:
  program: wc
  arguments: ["-l", "${path}"]
```

- **`name`** — lowercase ASCII letters, digits, and hyphens; must match the file
  stem.
- **`description`** — what the tool does; shown to the model as the tool's
  description.
- **`parameters`** — each has a `name` and a `description`. Every parameter is a
  required string, is exposed in the tool's JSON schema, and must be referenced
  by at least one argument. A declared-but-unused parameter, or an argument
  referencing an undeclared one, is a definition error.
- **`run.program`** — the fixed executable. It may **not** contain a `${...}`
  placeholder: a loaded extension always runs the same program, so a preview can
  always name it.
- **`run.arguments`** — the exact argv. `${name}` placeholders are substituted
  with the validated argument value. Substitution is whole-value into a single
  argv element — `"a file.txt"` stays one argument — because the argv reaches
  the operating system as written and there is no shell to re-split it. Inline
  substitution within a token (`"--file=${path}"`) is supported.

## Discovery, load, and gating

Discovery **lists** an extension; it does not make it callable. `/reload`
re-reads `.smed/extensions/` and reports what appeared or vanished alongside
skills and templates. A malformed file is reported with a typed reason and
skipped — it never half-registers.

There are two explicit acts that make a discovered extension callable, and both
record an `extension_loaded` event naming the tool, its fixed program, and what
authorised the load:

- **`/load-extension <name>`** — the human's direct command (authority
  `command`). It resolves the discovered definition, refusing an unknown or
  already-taken name without recording anything, and registers the tool.
- **The `load_extension` tool** — the agent loop proposing to extend itself
  (authority `full_auto`). Offered to the model only when extensions are
  discovered. Proposing a load is an `Execute`-tier act, so under full-auto it
  auto-resolves, and under `ask` a human sees it. A project-scoped extension
  adds the project-trust gate on top: `load_extension` reports
  `requires_workspace_trust`, so a model-proposed load of a project extension is
  held for a human **even under full-auto** — the guard that matters when no
  human typed the command. The tool only validates the name; the runtime
  performs the registration when the call completes, because only it holds the
  live tool registry.

The two paths share one registration core, so they cannot disagree about what
"loaded" means; the only difference is the recorded authority and, for the model
path, the trust gate.

Three gates stand around an extension, and loading relaxes none of them:

1. **Writing the file** goes through the ordinary `Write` tier, like any file.
2. **Loading** is the explicit, evidenced act above — never automatic.
3. **Every call** the loaded tool makes is gated at `Execute` tier and previewed
   as its exact argv, exactly like `run_command`. An extension cannot declare a
   lower tier; provenance is unknown, so it fails closed
   ([`tool-policy.md`](tool-policy.md)).

The human `/load-extension` needs no separate trust gate — the human is the
authorisation, and the command is gated on every call. The model's
`load_extension` adds the project-trust gate, because a model self-extending
from an untrusted project's file is exactly the case where a human should be
asked first.

## Scope and lifetime

A loaded extension applies to the session that loaded it. It is **not** persisted
as project configuration and **not** silently reloaded on resume: a resumed
session re-discovers the file but the load act must be repeated. The
`extension_loaded` event is evidence of what happened, not an instruction to
re-enable.

An extension can do exactly what `run_command` can do and no more — orchestrate a
bounded command at the workspace root. It cannot hold state, read files into the
model, or make network calls except by invoking a program that does, and every
such program is itself subject to the same `Execute` gate.
