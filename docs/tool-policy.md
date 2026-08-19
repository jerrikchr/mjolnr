# Tool and policy contract

Phase 3 implements smed's governed execution boundary. The model can propose an action; only deterministic Rust code validates, classifies, approves, and executes it.

## This is not a sandbox

smed provides policy gates, canonical-path containment, approval prompts, budgets, and process cancellation. It does **not** provide OS-level isolation. An approved command runs with the user's filesystem permissions and a scrubbed environment, so approval remains a meaningful security decision.

## Built-in tools

| Tool | Tier | Contract |
|---|---|---|
| `read_file` | read | Bounded UTF-8 line range; records the file's SHA-256 version. |
| `list_files` | read | Deterministic bounded listing; skips symlinks and heavy build/vendor directories. |
| `search_text` | read | Fixed-string search with `rg` when available and a bounded Rust fallback. |
| `write_file` | write | Creates a file, or replaces an existing file only after it was read and remains unchanged. |
| `edit_file` | write | Replaces exactly one exact string; fuzzy edits are not supported. Requires a current read version. |
| `run_command` | execute | Runs one explicit program plus argv at the workspace root; never assembles an implicit shell string. |
| `finish_task` | read | Reports `verified` or `unverified`, evidence event IDs, and remaining risks. |
| `plugin:<name>:<tool>` | execute | Third-party plugin tools are always `Execute` — every call gated, previewed, and evidenced (ADR-0016). |
| `mcp:<server>:<tool>` | execute | MCP tools are always `Execute` — every call gated (see `docs/mcp.md`). |

All argument schemas use JSON Schema Draft 2020-12 with local-only resolution. Arguments are validated when proposed and again immediately before `execute`.

## Policy modes

| Mode | Read | Write | Execute |
|---|---|---|---|
| `read-only` | allow | deny | deny |
| `ask` | allow | ask | ask |
| `workspace-write` | allow | allow inside workspace | ask |

An exact command approved with `a` is allowed again only for that exact program-and-argv tuple and only for the current session. There is no approve-all state. Unknown or missing tier classification resolves to `execute` and therefore requires the most restrictive gate.

Shift-Tab cycles the modes while no run is active. The active mode and consumed turn/tool budgets are visible in the header. Policy switching is locked during a run so a keystroke cannot silently change the rules governing work already in flight.

## Filesystem containment and stale reads

The workspace root is canonicalized when opened. Existing targets are canonicalized before use; new targets canonicalize their nearest existing ancestor. Parent-directory components, absolute paths outside the root, and symlinks resolving outside the root are refused.

Filesystem containment is rechecked immediately before each side effect. Existing files must first be read in the same session. smed stores their SHA-256 version and refuses the write if the file changed after that read.

## Approval and event ordering

`ToolProposed` is stored before any effect. If a human decision is required, `ApprovalResolved` is stored before the tool task starts. A denial becomes a structured `APPROVAL_DENIED` result returned to the model so it can choose another path.

The approval modal shows a bounded unified diff or the exact argv display. `y` approves once, `n` denies, and `a` approves only an exact command for this session.

## Commands, cancellation, and secrets

Commands run at the workspace root with null stdin, bounded stdout/stderr, a timeout, and a cancellation token. On Unix, smed starts a new process group; cancel sends TERM to the group and then KILL if it does not exit promptly. Provider variables ending in `_API_KEY` are removed from the child environment.

Non-zero exit status is a failed `TOOL_EXECUTION` result and includes the exit code. Timeouts use `COMMAND_TIMEOUT`. Truncation is disclosed in structured metadata and in bounded runtime output.

## Budgets and completion evidence

Defaults per run are 20 provider turns, 40 tool calls, 10 minutes wall time, 2 minutes per command, and 64 KiB per tool result. Exhaustion fails closed with `BUDGET_EXHAUSTED`.

After any successful mutation, `finish_task(outcome = "verified")` is accepted only when it cites a stored successful-command event created after the latest mutation. Without that evidence smed returns `COMPLETION_EVIDENCE_MISSING` to the model; it does not relabel the work as verified.
