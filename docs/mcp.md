# Governed MCP client

smed reads MCP servers only from `.smed/mcp.yaml` in the open workspace. It
does not scan editor configuration, discover executables, or connect over HTTP.

```yaml
servers:
  - name: local-tools
    command: /absolute/path/to/server
    args: ["--stdio"]
    pass_env: ["LOCAL_TOOLS_TOKEN"]
```

`name` is restricted to letters, numbers, `_`, and `-`. Environment values are
resolved from smed's process at spawn time; they are not stored in YAML,
logged, shown in `/mcp`, or copied into the transcript. The child environment
is cleared before the named variables are admitted. Secrets must never be put
in `args`, because command-line arguments are externally visible.

Discovered tools are registered as `mcp:<server>:<tool>`. Their declared safety
annotations are ignored: every MCP tool is `Execute`. Plugins are **not** MCP —
they are `plugin:<name>:<tool>` via [ADR-0016](adr/0016-plugin-protocol-and-capability-modules.md) (`smed plugin create` / `smed plugin list`, per-file `.smed/plugins/*.yaml`). End-to-end example `examples/plugins/vercel-deployments/`. Under `ask` this creates
the ordinary approval preview containing server, remote tool, and complete call
arguments. `read-only` refuses it. `full-auto` records the ordinary Phase 9
automatic-policy audit before the call. Native tools cannot be shadowed by an
MCP name.

Results pass through the same bounded tool-result path and durable events as
native tools. Child stderr is discarded at the transport boundary. `/mcp`
shows each configured server, connection state, governed tool count, tier, and
typed configuration/schema failure.
