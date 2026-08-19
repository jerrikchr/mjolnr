# Headless runs

`smed exec "<directive>"` creates one ordinary durable session, runs one
directive through the same runtime as the TUI, writes its history to the same
SQLite database, emits one JSON line, and exits.

```console
smed exec "inspect the release state"
smed exec "update generated files" --policy workspace-write
smed exec "run the governed migration" --policy full-auto
```

The default is `read-only`. Headless has no `ask` policy and no approval input
channel. If `workspace-write` reaches an Execute tool, smed resolves the
ordinary pending approval as denied and reports the same typed refusal the TUI
would return. `full-auto` remains explicit and retains containment,
read-before-edit, output, budget, quota, evidence, audit, and recovery guards.

Provider and model may be named together with `--provider` and `--model`; when
omitted, the same configured-provider preference as the TUI applies. The final
line has this stable shape:

```json
{"session_id":"...","outcome":"refused","exit_code":10,"reason_code":"APPROVAL_DENIED"}
```

| Exit | Outcome |
|---:|---|
| `0` | evidence-backed `verified` completion |
| `10` | typed policy, schema, or approval refusal |
| `20` | budget or provider-quota stop |
| `30` | provider, tool, durability, setup, or unverified-completion failure |

The session remains active and can be opened with
`smed --resume <session_id>`. Stdout contains only the JSON record, making the
command safe for CI parsers; diagnostics go to stderr before a report exists.
