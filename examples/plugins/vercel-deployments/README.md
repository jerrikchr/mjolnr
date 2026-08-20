# Vercel Deployments — Example ADR-0016 Plugin

Third-party plugin (not a capability module) demonstrating the ADR-0016
JSON-RPC 2.0 subprocess protocol end-to-end against the Vercel `TaskSource`
added in Phase 6.1.

## What it does

- **`list_deployments { project_id }`** — `GET /v6/deployments?projectId=…`
- **`get_deployment { deployment_id }`** — `GET /v6/deployments/{id}`
- **`session_start` hook** — announces availability as an observer annotation.

All tools are pinned `ToolTier::Execute` (ADR-0016 §3) and gated — no tier
self-declaration. Views are data-only (`view_type: table`).

## Install

```bash
# scaffold a copy to discover, or copy this example directly:
mjolnr plugin create vercel.deployments --template node --yes
# or manually:
mkdir -p .mjolnr/plugins
cp examples/plugins/vercel-deployments/mjolnr-plugin.yaml .mjolnr/plugins/vercel.deployments.yaml
```

Grant the credential (owner-only file, `0o600`, `Debug`-redacted):

```bash
# per-plugin credential file (see docs/plugins.md grant flow)
# then restart so the host spawns with VERCEL_TOKEN injected.
```

## Security

- Environment is `env_clear` + `sanitized_environment` + granted-only injection
  (`src/plugins/transport.rs`).
- Only `VERCEL_TOKEN` is injected; provider keys (`OPENAI_API_KEY`, etc.) are
  absent.
- `mjolnr-plugin.yaml` uses `required_credentials: ["VERCEL_TOKEN"]` — exact
  `UPPER_SNAKE` name, validated by `validate_credential_name`.

## Notes

- No SDK — `fetch` against `api.vercel.com` directly; wiremock in tests.
- `source_url` provenance: `https://github.com/jerrikchr/mjolnr/tree/main/examples/plugins/vercel-deployments`.
- Trust class: `THIRDPARTY · EXECUTE` badge in TUI `/plugins`.
