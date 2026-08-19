# mjolnr

mjolnr (short for Mjolnir, Thor's hammer) is a coding harness that lives in
your project and does the work with you: read, edit, run, and verify, in the
directory where you launched it. You see every change and every command before
it happens and after it lands. It talks directly to model provider APIs through
your own accounts, on hardware you own. That native path does not require
another coding-agent CLI.

The name draws from Norse mythology: forged by a dwarf and wielded by a god —
the model proposes, deterministic Rust disposes. The hammer returns after every
throw, just as the runtime re-establishes control after each model action.

> **Pre-release.** Not published to crates.io, not packaged for release. Build it
> from source; expect rough edges.

## Install

Requires Rust 1.97+ (edition 2024).

```bash
cargo install --path .
```

On macOS, `scripts/install.sh` installs and code-signs with a stable identity, so
the keychain stops re-asking for your password after every rebuild. Set
`SMED_SIGN_IDENTITY` to your own certificate (`security find-identity -v -p codesigning`).

## Quick start

```bash
mjolnr init               # guided setup: provider, model, roles, identity, theme
mjolnr auth login openai  # key read without echo, stored in an owner-only file
mjolnr                    # opens the console on a new session
```

`mjolnr init --yes` scaffolds `.mjolnr/` routing only, without prompts, for scripts and
CI. `--data-dir <path>` puts the session database somewhere disposable, which is the
easy way to try mjolnr without mixing it into real work.

## What it does

**In your repo**

- Bounded read, list, and search; read-before-edit writes; commands as exact argv.
- Every write and command is shown as a diff or argv before it runs, and approved
  one at a time. `y` approves once, `a` approves that exact command for the session.
- Policies you switch while idle: `read-only`, `ask`, `workspace-write`, and an
  explicitly armed `full-auto`.
- Completion is evidence-gated — "verified" requires a green command that ran after
  the last change, not the model's word for it.
- Governed local Git: status, branches, commits, worktrees, and governed
  `fetch`/`push`, each an approved act.

**Across sessions**

- History lives in one SQLite database. `mjolnr sessions list` and
  `mjolnr --resume <id>` reopen a session with its conversation, model, policy,
  budgets, read set, and tool results intact.
- `--compact` resumes from a stored handoff plus recent turns, keeping the full
  history on disk.
- `/tree` shows the session as a tree of turns; `/fork` and `/clone` branch from any
  point without touching the original. Abandoned branches stay readable.
- `mjolnr diagnostics` reports the database path, schema, WAL state, and session
  state; `sessions release <id>` reclaims a lease a crashed run left behind.

**Models and providers**

- OpenAI API, ChatGPT subscription (`openai-codex`), Anthropic, Gemini, OpenRouter,
  Ollama, and LM Studio.
- Deterministic route and role selection — a model per job, written down rather
  than learned, with typed conditions for advancing to the next hop.
- A multi-model council: ask several models the same question and review the
  responses side by side.
- Personas, and owner-declared per-model governance ceilings.

**Beyond one agent**

- Subagents in isolated worktrees, previewed and approved individually. A child's
  work lands on its own branch; merging is a separate act you take.
- Governed MCP, stdio and remote — every MCP tool passes the same gate.
- Bounded task sources as provider-neutral imports: GitHub and Linear (PRs + comments), Vercel (`api.vercel.com` deployments) and Supabase (`api.supabase.com` projects) read-only — all with `TaskId` charset validation, bounded reads, and `Secret`-redacted provenance. Runtime holds them in a `HashMap<IntegrationId, Arc<dyn TaskSource>>`, so adding a source is one registration line (`src/runtime/mod.rs:435`).
- Third-party plugins as isolated JSON-RPC subprocesses (ADR-0016): `mjolnr plugin create <name> [--template node|rust|python] [--yes]` scaffolds `.mjolnr/plugins/<name>.yaml` (+ starter), `mjolnr plugin list` discovers per-file manifests in `.mjolnr/plugins/*.yaml` and the user config dir; every rerun previews and **never overwrites** existing files. Flagship example `examples/plugins/vercel-deployments/` — `list_deployments`/`get_deployment`, `session_start` hook, `VERCEL_TOKEN` only via scrubbed env. All plugin tools pinned `ToolTier::Execute`.
- Agent-authored tool extensions (declarative argv templates — sibling system to plugins, see `docs/extensions.md` vs ADR-0016), loaded deliberately and run through the command gate.
- A deterministic Rust code graph for structural questions about the repo.

The native loop is the default, not a closed boundary around the workspace. The
roadmap includes external agent runtimes as clearly labelled collaborators. Their
output remains external and unverified until it crosses mjolnr's ordinary review,
policy, and evidence boundaries.

## Interfaces

The terminal console is what runs today: a Ratatui client over a Rust runtime core.
A Tauri desktop workspace lives in `desktop/` and shares that same runtime — it is
in active development and not yet packaged for release. Neither client owns an agent
loop; both consume the same runtime truth
([`ADR 0003`](docs/adr/0003-shared-rust-core-tui-tauri-clients.md)).

Press F1 or type `/help` in a session for the in-app control surface.

## Providers and credentials

Credentials are read without echo and stored in an owner-only file.

```bash
mjolnr auth login gemini      # Gemini API key
mjolnr auth login openrouter  # OpenRouter API key
mjolnr auth login lm-studio   # server IP/URL plus optional token
mjolnr auth status            # credential and local-provider readiness
```

`OPENAI_API_KEY` works too and takes precedence over a stored key. `openai-codex`
uses ChatGPT Plus/Pro subscription quota rather than API billing, and is a separate
provider rather than a mode on `openai`. Ollama uses `http://localhost:11434` and
needs no credential. LM Studio defaults to `http://localhost:1234`;
`mjolnr auth login lm-studio` asks for another address if your server is elsewhere and
writes it to `.mjolnr/providers/lm-studio.url`, overridable with
`SMED_LM_STUDIO_BASE_URL`.

`/model` lists only models actually discovered from connected providers, so a
catalogue failure shows as `needs re-auth` or `unavailable` rather than a stale
model. With no configured provider, a new session falls back to local Ollama.

## What it will not do

Stated up front, because the alternative is discovering it later:

- **It does not know whether a change is *correct*.** Evidence proves a command ran
  green after the last mutation — not that the edit was right. Review is still yours.
- **It does not currently provide an OS security sandbox.** The policy gate is
  approvals, path containment, and budgets. Stronger containment must be built and
  verified before smed claims it.
- **It will not guess what an interrupted command did.** Killed between approving a
  write and recording its outcome, smed says so and stops rather than re-running it
  or reporting an outcome it cannot demonstrate.
- **Approvals do not outlive the session.** `a` is never written to disk, and
  full-auto never survives a resume — it always reopens in `ask`.
- **Subscription OAuth is your call.** Logging in with a subscription account may
  conflict with a provider's terms; whether your use complies is your
  responsibility. API keys remain the canonical supported path.
- **Semantic recall is not implemented yet.** Sessions have durable history and
  explicit handoffs. Future recall layers must remain regenerable projections over
  the record and working tree, never a second source of authority.
- **Windows is unsupported** until it is actually tested there, and the database
  needs a local filesystem — a network share is not supported.

## Building

```bash
cargo build
cargo test          # no network, no credentials required
```

Full verification gate:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo deny check
```

A panic restores the terminal and writes a bounded redacted report beside smed's
database; provider text and panic payloads are intentionally excluded.

## Documentation

Start here:

| Document | Purpose |
|---|---|
| [`docs/POSITIONING.md`](./docs/POSITIONING.md) | What mjolnr is for, in one page. |
| [`AGENTS.md`](./AGENTS.md) | Engineering standards. Read before writing code. |
| [`docs/definition-of-done.md`](./docs/definition-of-done.md) | The finish line: the owner's whole workflow as a specification, and what "done" excludes. |
| [`docs/adr/`](./docs/adr/) | Architecture decision records. |

How it works:

| Document | Purpose |
|---|---|
| [`docs/tool-policy.md`](./docs/tool-policy.md) | Tool tiers and the policy gate. |
| [`docs/persistence.md`](./docs/persistence.md) | SQLite and `tokio-rusqlite` contracts, schema, recovery rules. |
| [`docs/provider-contract.md`](./docs/provider-contract.md) | Provider API contracts, marked confirmed vs inferred. |
| [`docs/mcp.md`](./docs/mcp.md) | Governed MCP, stdio and remote. |
| [`docs/headless.md`](./docs/headless.md) | Headless runs. |
| [`docs/extensions.md`](./docs/extensions.md) | Agent-authored tool extensions. |
| [`docs/tui-design.md`](./docs/tui-design.md) | Console visual language and the key-action contract. |
| [`docs/release.md`](./docs/release.md) | Release targets, artifact packaging, checksums, install instructions. |
| [`THIRD_PARTY.md`](./THIRD_PARTY.md) | Dependency inventory and licences. |

Where it is going:

| Document | Purpose |
|---|---|
| [`docs/ux-workspace-direction.md`](./docs/ux-workspace-direction.md) | Accepted product and interaction direction for a shared governed workspace across Ratatui and Tauri. |
| [`docs/tauri-path-and-phases.md`](./docs/tauri-path-and-phases.md) | Approved A0–C sequence: frontend-safe bridge, runtime workflow truth, bounded TUI closure, and Orca-like Tauri workspace. |
| [`docs/integrated-workspace-phases.md`](./docs/integrated-workspace-phases.md) | Approved D0–D12 sequence for hierarchy, exact review, search, Git/task integrations, editor, terminal/external agents, browser/design mode, SSH/accounts, and daily-driver verification. |
| [`docs/tauri-design-system.md`](./docs/tauri-design-system.md) | Desktop design-system contract, primitives, accessibility, performance, and Phase A0 exit criteria. |
| [`docs/master-implementation-plan.md`](./docs/master-implementation-plan.md) | Module/plugin runtime roadmap (P1–P6 landed: memory, plugin protocol, Fleet/Glassbox, preview+snapshots, TUI+collision, Vercel/Supabase + `mjolnr plugin create`). |
| [`docs/pre-release-checklist.md`](./docs/pre-release-checklist.md) | Publication gates — truth pass must cover task sources (GitHub/Linear/Vercel/Supabase) and plugin scaffold inventory. |

## Licence

[Apache License 2.0](./LICENSE). Copyright notice in [`NOTICE`](./NOTICE).

Dependencies are linked from crates.io under their own permissive licences, all of
which are inventoried in [`THIRD_PARTY.md`](./THIRD_PARTY.md) and enforced by
`cargo deny check`. No third-party source code is vendored into this repository.

Cargo packages remain `publish = false`: a crates.io release is a separate decision
that has not been made.
