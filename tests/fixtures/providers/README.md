# Provider fixtures — redaction and provenance rules

Fixtures are captured provider responses used by the contract tests. They are the reason the default test run needs no network and no credentials (`AGENTS.md` §7).

They are also the single most likely place for a credential or someone's source code to be committed by accident. These rules are not ceremony.

## Provenance — where a fixture may come from

**Allowed:**

- A response mjolnr captured from a live provider call made by us, against our own account, using a scratch prompt.
- A synthetic fixture handwritten from `docs/provider-contract.md` to exercise a specific decoder state (chunk splits, comment frames, unknown events, malformed tails).
- A literal example printed in official provider documentation, cited in the fixture's metadata.

**Forbidden:**

- Fixtures copied from another agent repository's test suite. `AGENTS.md` §8 forbids copying their tests, and a fixture is a test input. This is the rule most likely to be violated by convenience, because their fixtures are right there and already shaped correctly.
- Any capture containing a real repository's contents, a user's prompt, or anything not authored for this purpose.

## Redaction — non-negotiable

**Redact at capture time, never later.** A secret in an uncommitted file is a mistake; a secret in git history is an incident requiring key rotation. There is no "clean it up before committing" step, because that step is exactly the one that gets skipped.

Redact:

- API keys, bearer tokens, `Authorization` header values, `x-goog-api-key`, cookies, and anything in a URL query string (see `docs/provider-contract.md` §3.2 — Gemini documents the key as a query parameter).
- Organisation, project, and account identifiers.
- Request IDs and trace identifiers that tie back to a real account.
- Any prompt or completion content not written for the fixture.

Replace with a stable, obviously-fake placeholder so the shape survives and the value does not:

```text
sk-REDACTED-FIXTURE
Bearer REDACTED-FIXTURE
org-REDACTED
```

Placeholders must be *obviously* fake. `sk-abc123` looks like a real key that leaked; `sk-REDACTED-FIXTURE` cannot be mistaken for one.

Preserve everything a decoder is tested against: event names, field names, ordering, chunk boundaries, whitespace, and the exact framing bytes. A fixture that has been prettified is not a fixture — reformatting is how a decoder passes tests it would fail in production.

## Layout

```text
tests/fixtures/providers/
  openai/      anthropic/      gemini/      openrouter/      ollama/
```

Name by the behaviour under test, not by a ticket:

```text
anthropic/tool_use_input_json_delta_split_midkey.sse
openrouter/comment_frame_keepalive.sse
openrouter/midstream_error_http_200.sse
ollama/ndjson_chunk_boundary_midline.ndjson
openai/function_call_arguments_done.sse
```

## Metadata

Every fixture gets a sibling `.meta.toml`:

```toml
source = "live-capture"        # live-capture | synthetic | official-docs
captured = "2026-07-15"
provider_version = "2026-06-01"  # e.g. anthropic-version header, or API version
redacted = ["authorization", "request-id"]
proves = "partial_json accumulates across a key boundary; parse only at content_block_stop"
notes = "Split points chosen to land mid-escape-sequence."
```

`proves` is the important field. A fixture whose purpose nobody can state is a fixture nobody can safely change when a provider drifts.

## Before committing a fixture

1. Grep it for your own key material — `rg -i 'sk-|bearer|api[-_]?key|authorization'` — and confirm every hit is a placeholder.
2. Confirm no real repository content or personal prompt text survives.
3. Confirm the framing bytes are unmodified.
4. Confirm the `.meta.toml` exists and `proves` is a real sentence.

## When a provider drifts

A fixture is a snapshot of a contract that will change. When a live smoke test disagrees with a fixture, **the live provider is right and the fixture is stale.** Recapture it, update `docs/provider-contract.md`, and note the drift in the report. Do not edit a fixture to match the code's current behaviour — that inverts the test into a mirror and it will never fail again.
